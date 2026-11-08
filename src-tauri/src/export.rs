use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use futures::StreamExt;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use std::process::Stdio;
use tokio::process::{Child, Command};

use crate::ffmpeg::{self, Binaries, EncodeSpec};
use crate::project::CACHE_DIR;
use crate::state::ExportCtl;
use crate::timeline::{self, Plan};

pub const CANCELLED: &str = "cancelled";
/// Appended to stage errors that look like the GPU encoder failed to open,
/// so the export loop can retry that stage on the CPU.
const ENCODER_INIT_MARKER: &str = "\u{1}encoder-init";
const LIST_FILE: &str = "list.txt";
const STAGE_TIMEOUT: Duration = Duration::from_secs(900);

fn encoder_from_u8(value: u8) -> ffmpeg::Encoder {
    match value {
        1 => ffmpeg::Encoder::Nvenc,
        2 => ffmpeg::Encoder::Amf,
        3 => ffmpeg::Encoder::VideoToolbox,
        4 => ffmpeg::Encoder::Qsv,
        5 => ffmpeg::Encoder::Vaapi,
        _ => ffmpeg::Encoder::Libx264,
    }
}

fn strip_encoder_marker(error: &str) -> String {
    error
        .split(ENCODER_INIT_MARKER)
        .next()
        .unwrap_or(error)
        .to_string()
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProgressEvent {
    pub phase: String,
    pub index: usize,
    pub total: usize,
    pub label: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ExportSummary {
    pub out_path: String,
    pub total_duration: f64,
    pub encoder: String,
}

pub fn escape_concat_path(path: &Path) -> String {
    path.display().to_string().replace('\'', "'\\''")
}

/// Bump when segment encoding semantics change so old cache entries are not reused.
const SEGMENT_PIPELINE: &str = "segs-v2-tv-range";

/// Cache key for one normalized segment: everything that changes the encoded
/// result — the pipeline version, the asset file itself (size+mtime), the frame
/// spec, and the item choices. Reordering or re-exporting reuses every segment
/// that still hashes the same.
fn segment_hash(media: &Path, spec: &EncodeSpec, kind: &str, duration: f64, trim_start: f64) -> String {
    let meta = std::fs::metadata(media).ok();
    let size = meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut hasher = Sha256::new();
    hasher.update(SEGMENT_PIPELINE.as_bytes());
    hasher.update(media.as_os_str().to_string_lossy().as_bytes());
    hasher.update(size.to_le_bytes());
    hasher.update(mtime.to_le_bytes());
    hasher.update(format!(
        "{}x{}@{} crf{} {} {} {} {} dur{} trim{}",
        spec.width,
        spec.height,
        spec.fps,
        spec.crf,
        spec.preset,
        serde_json::to_string(&spec.fit).unwrap_or_default(),
        serde_json::to_string(&spec.background).unwrap_or_default(),
        kind,
        (duration * 1000.0).round() as i64,
        (trim_start * 1000.0).round() as i64,
    ));
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

async fn run_ffmpeg_stage(
    bins: &Binaries,
    args: &[String],
    ctl: &ExportCtl,
    stage_name: &str,
) -> Result<(), String> {
    if ctl.cancelled() {
        return Err(CANCELLED.to_string());
    }
    let mut command = Command::new(&bins.ffmpeg);
    command
        .args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child: Child = command
        .spawn()
        .map_err(|e| format!("Could not start the video engine ({e})."))?;
    let mut stderr = child.stderr.take().expect("stderr was piped");
    let child_id = ctl.attach(child);

    let mut tail: Vec<u8> = Vec::new();
    let mut head: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    let started = Instant::now();
    let status = loop {
        if ctl.cancelled() {
            ctl.detach(child_id);
            return Err(CANCELLED.to_string());
        }
        match tokio::time::timeout(Duration::from_millis(60), stderr.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => {
                // Output pipe closed: the process is on its way out.
            }
            Ok(Ok(n)) => {
                if head.len() < 8192 {
                    head.extend_from_slice(&buf[..n]);
                }
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > 8192 {
                    tail = tail.split_off(tail.len() - 4096);
                }
            }
            Err(_) => {}
        }
        if let Some(result) = ctl.try_wait(child_id) {
            break result.map_err(|e| format!("Could not finish {stage_name}: {e}"))?;
        }
        if started.elapsed() > STAGE_TIMEOUT {
            ctl.detach(child_id);
            return Err(format!("{stage_name} took too long and was stopped."));
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    };
    ctl.detach(child_id);

    if !status.success() {
        let detail = String::from_utf8_lossy(&tail);
        // Encoder-init lines may be scrolled out of the retained tail, so the
        // first 8 KB is scanned as well.
        let encoder_init = {
            let head = String::from_utf8_lossy(&head);
            let both = format!("{head}\n{detail}");
            both.contains("Error while opening encoder")
                || both.contains("Could not open encoder")
                || both.contains("No capable devices")
                || both.contains("cuCtxCreate")
                || both.contains("InitializeEncoder failed")
        };
        eprintln!("ffmpeg stage {stage_name:?} failed:\n{detail}");
        let last_line = detail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("unknown error");
        let message = format!("{stage_name} failed: {last_line}");
        return Err(if encoder_init {
            format!("{message}{ENCODER_INIT_MARKER}")
        } else {
            message
        });
    }
    Ok(())
}

/// Full export: per-item cached segments -> lossless concat -> optional music mux
/// -> faststart MP4. Assumes `ctl.begin()` was called by the caller.
pub async fn export_project(
    bins: &Binaries,
    ctx: &crate::project::ProjectCtx,
    out_path: &Path,
    ctl: &ExportCtl,
    on_progress: &mut (dyn FnMut(&ProgressEvent) + Send),
) -> Result<ExportSummary, String> {
    let plan = timeline::plan(ctx)?;
    let total = plan.total_duration;
    if plan.items.is_empty() {
        return Err("Add some photos before creating the video.".to_string());
    }

    let cache_dir = ctx.dir.join(CACHE_DIR);
    std::fs::create_dir_all(&cache_dir)
        .map_err(|e| format!("Could not prepare the slideshow folder: {e}"))?;
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not prepare the output folder: {e}"))?;
    }

    // Encoder preference + runtime probe: a GPU encoder that fails to
    // initialize (broken driver session) never blocks an export.
    let encoder = ffmpeg::resolve_encoder(bins, &ctx.project.settings.encoder);
    println!("[export] encoder: {}", encoder.label());

    // Segments. Items sharing a hash encode once; independent segments encode
    // in parallel (bounded) — per-segment cost is dominated by image decode +
    // scale, which parallelizes well across cores.
    let total_items = plan.items.len();
    let mut list_entries: Vec<String> = Vec::with_capacity(total_items);
    #[derive(Clone)]
    struct ItemInfo {
        kind: &'static str,
        asset_name: String,
    }
    struct JobData {
        media: PathBuf,
        spec: ffmpeg::EncodeSpec,
        segment_kind: ffmpeg::SegmentKind,
        duration: f64,
    }
    let mut entries: Vec<(String, ItemInfo)> = Vec::with_capacity(total_items); // (hash, info)
    let mut job_data: std::collections::HashMap<String, JobData> =
        std::collections::HashMap::new();

    for planned in plan.items.iter() {
        let item = ctx
            .project
            .items
            .iter()
            .find(|it| it.id == planned.item_id)
            .ok_or("The slideshow changed while exporting.")?;
        let spec = ffmpeg::EncodeSpec {
            width: ctx.project.settings.width,
            height: ctx.project.settings.height,
            fps: ctx.project.settings.fps,
            crf: ctx.project.settings.crf,
            preset: ctx.project.settings.preset.clone(),
            encoder,
            fit: item.fit,
            background: ctx.project.settings.background,
        };
        let kind = match planned.kind {
            crate::project::AssetKind::Image => "image",
            crate::project::AssetKind::Video => "video",
            crate::project::AssetKind::Audio => unreachable!("plan() rejects audio items"),
        };
        let media = Path::new(&planned.media_path);
        let hash = segment_hash(media, &spec, kind, planned.duration, planned.trim_start);
        if job_data.contains_key(&hash) || cache_dir.join(format!("{hash}.ts")).exists() {
            entries.push((hash, ItemInfo { kind, asset_name: String::new() }));
            continue;
        }
        let segment_kind = match planned.kind {
            crate::project::AssetKind::Video => ffmpeg::SegmentKind::Video {
                trim_start: planned.trim_start,
            },
            _ => ffmpeg::SegmentKind::Image,
        };
        let name = media
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "photo".to_string());
        job_data.insert(
            hash.clone(),
            JobData {
                media: media.to_path_buf(),
                spec,
                segment_kind,
                duration: planned.duration,
            },
        );
        entries.push((hash, ItemInfo { kind, asset_name: name }));
    }

    // Jobs in stable (first-occurrence) item order; completions arrive in any
    // order and each emits one "preparing" tick so the bar never stalls.
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut jobs: Vec<(String, ItemInfo, JobData)> = Vec::new();
    for (hash, info) in entries.iter() {
        if job_data.contains_key(hash) && seen.insert(hash.clone()) {
            jobs.push((hash.clone(), info.clone(), job_data.remove(hash).unwrap()));
        }
    }

    let cpu_cap = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4);
    // GPU encoder sessions are limited (consumer GPUs cap concurrent NVENC
    // sessions), so run fewer stages at once when encoding on the GPU.
    let worker_cap = if encoder == ffmpeg::Encoder::Libx264 {
        cpu_cap.min(4)
    } else {
        4
    };
    let workers = jobs.len().min(worker_cap);
    let job_count = jobs.len();

    // Live encoder choice. If GPU encoding fails mid-export (VRAM eaten by
    // another app, driver hiccup), the first failure switches the whole
    // remaining export to the processor and retries the failed segment.
    let active_encoder = std::sync::atomic::AtomicU8::new(match encoder {
        ffmpeg::Encoder::Libx264 => 0,
        ffmpeg::Encoder::Nvenc => 1,
        ffmpeg::Encoder::Amf => 2,
        ffmpeg::Encoder::VideoToolbox => 3,
        ffmpeg::Encoder::Qsv => 4,
        ffmpeg::Encoder::Vaapi => 5,
    });
    let cache = &cache_dir;
    let active = &active_encoder;
    let mut prepared = total_items - job_count;
    if prepared > 0 {
        // Cached / duplicate items are already done — credit them up front.
        on_progress(&ProgressEvent {
            phase: "preparing".to_string(),
            index: prepared,
            total: total_items,
            label: format!("Reusing {prepared} of {total_items} prepared pieces…"),
        });
    }
    if job_count > 0 {
        let mut stream = futures::stream::iter(jobs.into_iter().map(|(hash, info, data)| {
            let segment = cache.join(format!("{hash}.ts"));
            async move {
                if ctl.cancelled() {
                    return Err(CANCELLED.to_string());
                }
                // Encode to a .part file, then rename, so an aborted run never
                // leaves a truncated segment that looks cached.
                let staging = cache.join(format!("{hash}.ts.part"));
                let stage = if info.kind == "image" { "photo" } else { "video" };
                let build_args = |enc: ffmpeg::Encoder| {
                    let mut spec = data.spec.clone();
                    spec.encoder = enc;
                    ffmpeg::segment_command(
                        &data.media,
                        &staging,
                        &spec,
                        data.segment_kind,
                        data.duration,
                    )
                };
                let mut enc = encoder_from_u8(active.load(std::sync::atomic::Ordering::SeqCst));
                let mut args = build_args(enc);
                let mut result =
                    run_ffmpeg_stage(bins, &args, ctl, &format!("Preparing {stage}")).await;
                if result.as_ref().is_err_and(|e| {
                    e.contains(ENCODER_INIT_MARKER) && enc != ffmpeg::Encoder::Libx264
                }) {
                    let _ = std::fs::remove_file(&staging);
                    active.store(0, std::sync::atomic::Ordering::SeqCst);
                    println!(
                        "[export] GPU encoding failed mid-export; finishing on the processor."
                    );
                    enc = ffmpeg::Encoder::Libx264;
                    args = build_args(enc);
                    result =
                        run_ffmpeg_stage(bins, &args, ctl, &format!("Preparing {stage}")).await;
                }
                match result {
                    Ok(()) => {
                        let tiny =
                            std::fs::metadata(&staging).map(|m| m.len() < 1024).unwrap_or(true);
                        if tiny {
                            let _ = std::fs::remove_file(&staging);
                            return Err(format!(
                                "“{}” could not be turned into video.",
                                info.asset_name
                            ));
                        }
                        std::fs::rename(&staging, &segment)
                            .map_err(|e| format!("Could not save the prepared segment: {e}"))?;
                        Ok(())
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&staging);
                        Err(strip_encoder_marker(&e))
                    }
                }
            }
        }))
        .buffer_unordered(workers);

        while let Some(result) = stream.next().await {
            if let Err(e) = result {
                drop(stream);
                return Err(e);
            }
            prepared += 1;
            on_progress(&ProgressEvent {
                phase: "preparing".to_string(),
                index: prepared,
                total: total_items,
                label: format!("Preparing {prepared} of {total_items}…"),
            });
        }
    }

    if ctl.cancelled() {
        return Err(CANCELLED.to_string());
    }
    for (hash, _) in entries.iter() {
        list_entries.push(format!("file '{}'", escape_concat_path(&cache_dir.join(format!("{hash}.ts")))));
    }

    // Concat (stream copy)
    on_progress(&ProgressEvent {
        phase: "stitching".to_string(),
        index: total_items,
        total: total_items,
        label: "Putting your slideshow together…".to_string(),
    });
    let list_file = cache_dir.join(LIST_FILE);
    std::fs::write(&list_file, list_entries.join("\n") + "\n")
        .map_err(|e| format!("Could not write the timeline list: {e}"))?;
    let merged = cache_dir.join("merged.ts");
    run_ffmpeg_stage(
        bins,
        &ffmpeg::concat_command(&list_file, &merged),
        ctl,
        "Stitching",
    )
    .await?;

    // Music mux or plain remux
    let audio = ctx.project.audio.clone();
    if let Some(audio) = &audio {
        let song_asset = ctx
            .project
            .assets
            .get(&audio.asset_id)
            .ok_or("The chosen song is missing from the folder.")?;
        let song = ctx.media_path(song_asset);
        on_progress(&ProgressEvent {
            phase: "music".to_string(),
            index: total_items,
            total: total_items,
            label: "Adding your music…".to_string(),
        });
        run_ffmpeg_stage(
            bins,
            &ffmpeg::mux_command(
                &merged,
                &song,
                out_path,
                total,
                audio.volume,
                audio.fade_in,
                audio.fade_out,
                audio.loop_track,
            ),
            ctl,
            "Adding music",
        )
        .await?;
    } else {
        on_progress(&ProgressEvent {
            phase: "finishing".to_string(),
            index: total_items,
            total: total_items,
            label: "Finishing up…".to_string(),
        });
        run_ffmpeg_stage(
            bins,
            &ffmpeg::remux_command(&merged, out_path),
            ctl,
            "Finishing",
        )
        .await?;
    }

    let _ = std::fs::remove_file(&merged);
    let _ = std::fs::remove_file(&list_file);

    Ok(ExportSummary {
        out_path: out_path.display().to_string(),
        total_duration: total,
        encoder: encoder_from_u8(active_encoder.load(std::sync::atomic::Ordering::SeqCst))
            .label()
            .to_string(),
    })
}

/// Plan only (no side effects) — used by tests and future preview caching.
pub fn build_plan(ctx: &crate::project::ProjectCtx) -> Result<Plan, String> {
    timeline::plan(ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn concat_list_escapes_quotes_in_paths() {
        let escaped = escape_concat_path(Path::new("/tmp/it's here.ts"));
        assert_eq!(escaped, "/tmp/it'\\''s here.ts");
    }

    #[test]
    fn segment_hash_changes_with_item_choices() {
        let media = Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
        let spec = || EncodeSpec {
            width: 1920,
            height: 1080,
            fps: 30,
            crf: 21,
            preset: "medium".to_string(),
            encoder: crate::ffmpeg::Encoder::Libx264,
            fit: crate::project::Fit::Contain,
            background: crate::project::Background::Black,
        };
        let a = segment_hash(&media, &spec(), "image", 3.0, 0.0);
        let b = segment_hash(&media, &spec(), "image", 4.0, 0.0);
        let c = segment_hash(&media, &spec(), "image", 3.0, 0.0);
        assert_ne!(a, b, "different durations must not share a cache entry");
        assert_eq!(a, c, "same inputs must reuse the cache entry");
        assert_eq!(a.len(), 64);
    }
}
