use std::path::Path;
use std::time::{Duration, Instant, SystemTime};

use serde::Serialize;
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use std::process::Stdio;
use tokio::process::{Child, Command};

use crate::ffmpeg::{self, Binaries, EncodeSpec, SegmentKind};
use crate::project::CACHE_DIR;
use crate::state::ExportCtl;
use crate::timeline::{self, Plan};

pub const CANCELLED: &str = "cancelled";
const LIST_FILE: &str = "list.txt";
const STAGE_TIMEOUT: Duration = Duration::from_secs(900);

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
    ctl.attach(child);

    let mut tail: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    let started = Instant::now();
    let status = loop {
        if ctl.cancelled() {
            ctl.detach();
            return Err(CANCELLED.to_string());
        }
        match tokio::time::timeout(Duration::from_millis(60), stderr.read(&mut buf)).await {
            Ok(Ok(0)) | Ok(Err(_)) => {
                // Output pipe closed: the process is on its way out.
            }
            Ok(Ok(n)) => {
                tail.extend_from_slice(&buf[..n]);
                if tail.len() > 8192 {
                    tail = tail.split_off(tail.len() - 4096);
                }
            }
            Err(_) => {}
        }
        if let Some(result) = ctl.try_wait() {
            break result.map_err(|e| format!("Could not finish {stage_name}: {e}"))?;
        }
        if started.elapsed() > STAGE_TIMEOUT {
            ctl.detach();
            return Err(format!("{stage_name} took too long and was stopped."));
        }
        tokio::time::sleep(Duration::from_millis(80)).await;
    };
    ctl.detach();

    if !status.success() {
        let detail = String::from_utf8_lossy(&tail);
        let last_line = detail
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("unknown error");
        return Err(format!("{stage_name} failed: {last_line}"));
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

    // Segments
    let total_items = plan.items.len();
    let mut list_entries: Vec<String> = Vec::with_capacity(total_items);
    for (i, planned) in plan.items.iter().enumerate() {
        if ctl.cancelled() {
            return Err(CANCELLED.to_string());
        }
        let item = ctx
            .project
            .items
            .iter()
            .find(|it| it.id == planned.item_id)
            .ok_or("The slideshow changed while exporting.")?;
        let spec = EncodeSpec {
            width: ctx.project.settings.width,
            height: ctx.project.settings.height,
            fps: ctx.project.settings.fps,
            crf: ctx.project.settings.crf,
            preset: ctx.project.settings.preset.clone(),
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
        let segment = cache_dir.join(format!("{hash}.ts"));

        if !segment.exists() {
            let kind_word = if kind == "image" { "photo" } else { "video" };
            on_progress(&ProgressEvent {
                phase: "preparing".to_string(),
                index: i + 1,
                total: total_items,
                label: format!("Preparing {kind_word} {} of {total_items}…", i + 1),
            });
            let args = ffmpeg::segment_command(
                media,
                &segment,
                &spec,
                match planned.kind {
                    crate::project::AssetKind::Video => SegmentKind::Video {
                        trim_start: planned.trim_start,
                    },
                    _ => SegmentKind::Image,
                },
                planned.duration,
            );
            run_ffmpeg_stage(bins, &args, ctl, &format!("Preparing {kind_word} {}", i + 1)).await?;
            let tiny = std::fs::metadata(&segment).map(|m| m.len() < 1024).unwrap_or(true);
            if tiny {
                let _ = std::fs::remove_file(&segment);
                return Err(format!(
                    "“{}” could not be turned into video.",
                    item.asset_id
                ));
            }
        }
        list_entries.push(format!("file '{}'", escape_concat_path(&segment)));
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
