use std::path::{Path, PathBuf};

use crate::project::{Background, Fit};

/// Resolved ffmpeg/ffprobe binaries. Resolution order:
/// LANTERN_FFMPEG/LANTERN_FFPROBE env vars, sidecar next to the executable
/// (installed layout from `bundle.externalBin`), resource dir, `binaries/` relative
/// to the cwd (`tauri dev` runs from src-tauri), then PATH.
pub struct Binaries {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Video encoder backend. Selected from the user's preference plus a runtime
/// probe (some GPUs/drivers expose an encoder that then fails at init, e.g.
/// broken NVIDIA driver sessions — the probe catches that and falls back).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Encoder {
    Libx264,
    Nvenc,
    Amf,
    VideoToolbox,
    Qsv,
    Vaapi,
}

impl Encoder {
    pub fn name(&self) -> &'static str {
        match self {
            Encoder::Libx264 => "libx264",
            Encoder::Nvenc => "h264_nvenc",
            Encoder::Amf => "h264_amf",
            Encoder::VideoToolbox => "h264_videotoolbox",
            Encoder::Qsv => "h264_qsv",
            Encoder::Vaapi => "h264_vaapi",
        }
    }

    /// Short label for logs and the export summary.
    pub fn label(&self) -> &'static str {
        match self {
            Encoder::Libx264 => "processor",
            Encoder::Nvenc => "graphics card (NVIDIA)",
            Encoder::Amf => "graphics card (AMD)",
            Encoder::VideoToolbox => "graphics card (Apple)",
            Encoder::Qsv => "graphics card (Intel Quick Sync)",
            Encoder::Vaapi => "graphics card",
        }
    }

    /// Encoders that need frames uploaded to GPU memory via the filter graph.
    pub fn needs_hwupload(&self) -> bool {
        matches!(self, Encoder::Qsv | Encoder::Vaapi)
    }
}

/// Runs a ~0.2 s black-frame encode with the same encoder flags the real
/// export uses, so a probe that passes guarantees the stage flags parse.
fn encoder_works(bins: &Binaries, encoder: Encoder) -> bool {
    let quality_flags: &[&str] = match encoder {
        Encoder::Libx264 => &["-preset", "medium", "-crf", "21"],
        Encoder::Nvenc => &["-preset", "p5", "-rc", "constqp", "-cq", "21"],
        Encoder::Amf => &["-quality", "balanced", "-rc", "cqp", "-qp_i", "21", "-qp_p", "21"],
        Encoder::VideoToolbox => &["-q:v", "60"],
        Encoder::Qsv => &["-rc", "cqp", "-global_quality", "21"],
        Encoder::Vaapi => &["-rc", "cqp", "-global_quality", "21"],
    };
    let mut command = std::process::Command::new(&bins.ffmpeg);
    command.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        // Production-scale probe: tiny probes can pass while real 1080p GPU
        // contexts fail (driver session/VRAM limits), which is exactly the
        // failure this probe exists to catch.
        "color=black:s=1920x1080:r=30:d=0.5",
    ]);
    command.arg("-c:v").arg(encoder.name());
    command.args(quality_flags);
    if encoder.needs_hwupload() {
        // Same upload path the real export uses (the default hardware device
        // is opened automatically).
        command.args(["-vf", "format=nv12,hwupload"]);
    } else {
        command.args(["-pix_fmt", "yuv420p"]);
    }
    command.args(["-f", "null", "-"]);
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// Which GPU encoder (if any) this machine can actually use. Probed once per
/// process, best-known candidates first (NVIDIA, Apple, Intel QSV, AMD, VAAPI).
fn gpu_encoder(bins: &Binaries) -> Option<Encoder> {
    static CACHE: std::sync::OnceLock<Option<Encoder>> = std::sync::OnceLock::new();
    *CACHE.get_or_init(|| {
        [
            Encoder::Nvenc,
            Encoder::VideoToolbox,
            Encoder::Qsv,
            Encoder::Amf,
            Encoder::Vaapi,
        ]
        .into_iter()
        .find(|encoder| encoder_works(bins, *encoder))
    })
}

/// Map the persisted `"auto" | "cpu" | "gpu"` preference to a working encoder.
/// `"gpu"` still falls back to CPU when the GPU encoder is broken or absent —
/// an export must never fail just because a preference was optimistic.
pub fn resolve_encoder(bins: &Binaries, preference: &str) -> Encoder {
    match preference {
        "auto" | "gpu" => gpu_encoder(bins).unwrap_or(Encoder::Libx264),
        _ => Encoder::Libx264,
    }
}

impl Binaries {
    pub fn resolve(resource_dir: Option<&Path>) -> Binaries {
        Binaries {
            ffmpeg: resolve_tool("ffmpeg", "LANTERN_FFMPEG", resource_dir),
            ffprobe: resolve_tool("ffprobe", "LANTERN_FFPROBE", resource_dir),
        }
    }
}

/// Target triple used in sidecar binary naming, e.g. "x86_64-unknown-linux-gnu".
pub fn target_triple() -> String {
    let platform = match std::env::consts::OS {
        "linux" => "unknown-linux-gnu",
        "macos" => "apple-darwin",
        "windows" => "pc-windows-msvc",
        other => other,
    };
    format!("{}-{}", std::env::consts::ARCH, platform)
}

fn tool_file_name(base: &str) -> String {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    format!("{base}-{}{}", target_triple(), exe)
}

fn candidate_paths(base: &str, resource_dir: Option<&Path>) -> Vec<PathBuf> {
    let exe = if cfg!(windows) { ".exe" } else { "" };
    let mut names = vec![tool_file_name(base)];
    // A universal macOS bundle ships one ffmpeg-universal-apple-darwin sidecar
    // while the running process reports an aarch64 or x86_64 triple.
    if cfg!(target_os = "macos") {
        names.push(format!("{base}-universal-apple-darwin{exe}"));
    }
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(exe_dir) = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
    {
        dirs.push(exe_dir.join("binaries"));
        dirs.push(exe_dir);
    }
    if let Some(resource_dir) = resource_dir {
        dirs.push(resource_dir.join("binaries"));
        dirs.push(resource_dir.to_path_buf());
    }
    dirs.push(PathBuf::from("binaries"));
    dirs.iter().flat_map(|dir| names.iter().map(|n| dir.join(n))).collect()
}

fn resolve_tool(base: &str, env_var: &str, resource_dir: Option<&Path>) -> PathBuf {
    if let Ok(value) = std::env::var(env_var) {
        if !value.is_empty() {
            return PathBuf::from(value);
        }
    }
    if let Some(path) = candidate_paths(base, resource_dir).into_iter().find(|p| p.exists()) {
        return path;
    }
    PathBuf::from(base) // PATH fallback
}

pub fn parse_version_line(stdout: &str) -> Option<String> {
    let line = stdout.lines().find(|l| !l.trim().is_empty())?;
    let mut words = line.split_whitespace();
    if words.next()? != "ffmpeg" || words.next()? != "version" {
        return None;
    }
    words.next().map(String::from)
}

pub async fn run_version(ffmpeg: &Path) -> Result<String, String> {
    let output = tokio::process::Command::new(ffmpeg)
        .arg("-version")
        .output()
        .await
        .map_err(|e| format!("{ffmpeg:?} failed to spawn: {e}"))?;
    if !output.status.success() {
        return Err(format!("{ffmpeg:?} -version failed to run"));
    }
    parse_version_line(&String::from_utf8_lossy(&output.stdout)).ok_or_else(|| {
        format!(
            "unrecognized ffmpeg version output: {}",
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .next()
                .unwrap_or_default()
        )
    })
}

/// Everything the segment encoder needs: frame spec from project settings plus the
/// per-item choices. Kept separate from project types so command builders stay pure.
#[derive(Clone)]
pub struct EncodeSpec {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub crf: u32,
    pub preset: String,
    pub encoder: Encoder,
    pub fit: Fit,
    pub background: Background,
    /// Ken Burns zoom-in motion on image segments (videos are unaffected).
    pub kenburns: bool,
}

/// ffmpeg options for one asset -> one normalized MPEG-TS segment.
pub enum FilterPlan {
    /// Single-input chain passed via `-vf`.
    Simple(String),
    /// Multi-pad graph passed via `-filter_complex`, mapped via `[vout]`.
    Complex(String),
}

pub fn format_seconds(value: f64) -> String {
    format!("{:.3}", value)
}

fn background_color(background: Background) -> &'static str {
    match background {
        Background::Black => "black",
        Background::White => "white",
        Background::Blur => "black", // blur replaces the pad background entirely
    }
}

/// Normalizing tail every segment shares: even SAR (phone video SAR varies and
/// breaks concat), constant frame rate, yuv420p.
fn tail(spec: &EncodeSpec) -> String {
    let upload = if spec.encoder.needs_hwupload() {
        ",format=nv12,hwupload"
    } else {
        ",format=yuv420p"
    };
    format!(",setsar=1,fps={}{}", spec.fps, upload)
}

pub fn segment_filter(spec: &EncodeSpec) -> FilterPlan {
    let w = spec.width;
    let h = spec.height;
    match (spec.fit, spec.background) {
        (Fit::Cover, _) => FilterPlan::Simple(format!(
            "scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}{}",
            tail(spec)
        )),
        (Fit::Contain, Background::Blur) => FilterPlan::Complex(format!(
            concat!(
                "[0:v]split=2[fg][bg];",
                "[bg]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},gblur=sigma=20[bgb];",
                "[fg]scale={w}:{h}:force_original_aspect_ratio=decrease[fgs];",
                "[bgb][fgs]overlay=(W-w)/2:(H-h)/2,setsar=1,fps={fps}{upload}[vout]"
            ),
            w = w,
            h = h,
            fps = spec.fps,
            upload = if spec.encoder.needs_hwupload() {
                ",format=nv12,hwupload"
            } else {
                ",format=yuv420p"
            }
        )),
        (Fit::Contain, background) => FilterPlan::Simple(format!(
            concat!(
                "scale={w}:{h}:force_original_aspect_ratio=decrease,",
                "pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color={bg}{tail}"
            ),
            w = w,
            h = h,
            bg = background_color(background),
            tail = tail(spec)
        )),
    }
}

/// Ken Burns: scale the normalized frame up to 2x the target first, then let
/// zoompan zoom the centre window from 1.0 to 1.18 over the segment. `d` is
/// the exact frame count so zoompan itself generates every output frame — no
/// tpad cloning, and `fps` makes the output constant-rate. The step is chosen
/// so the zoom reaches 1.18 exactly on the last frame.
fn kenburns_filter(spec: &EncodeSpec, frames: i64) -> FilterPlan {
    let w = spec.width;
    let h = spec.height;
    let w2 = w * 2;
    let h2 = h * 2;
    let zstep = 0.18 / (frames.max(1) as f64);
    let zoom = format!(
        "zoompan=z='min(zoom+{zstep:.6},1.18)':d={frames}:x='iw/2-(iw/zoom/2)':y='ih/2-(ih/zoom/2)':s={w}x{h}:fps={}",
        spec.fps
    );
    // zoompan already fixes size and rate, so the tail is the SAR/range/format
    // end only; the muxer's `-r` keeps the stream constant-frame-rate.
    let upload = if spec.encoder.needs_hwupload() {
        ",format=nv12,hwupload"
    } else {
        ",format=yuv420p"
    };
    match (spec.fit, spec.background) {
        (Fit::Cover, _) => FilterPlan::Simple(format!(
            "scale={w2}:{h2}:force_original_aspect_ratio=increase,crop={w2}:{h2},{zoom},setsar=1{upload}"
        )),
        (Fit::Contain, Background::Blur) => FilterPlan::Complex(format!(
            concat!(
                "[0:v]split=2[fg][bg];",
                "[bg]scale={w2}:{h2}:force_original_aspect_ratio=increase,crop={w2}:{h2},gblur=sigma=20[bgb];",
                "[fg]scale={w2}:{h2}:force_original_aspect_ratio=decrease[fgs];",
                "[bgb][fgs]overlay=(W-w)/2:(H-h)/2,{zoom},setsar=1{upload}[vout]"
            ),
            w2 = w2,
            h2 = h2,
            zoom = zoom,
            upload = upload
        )),
        (Fit::Contain, background) => FilterPlan::Simple(format!(
            concat!(
                "scale={w2}:{h2}:force_original_aspect_ratio=decrease,",
                "pad={w2}:{h2}:(ow-iw)/2:(oh-ih)/2:color={bg},",
                "{zoom},setsar=1{upload}"
            ),
            w2 = w2,
            h2 = h2,
            bg = background_color(background),
            zoom = zoom,
            upload = upload
        )),
    }
}

#[derive(Clone, Copy)]
pub enum SegmentKind {
    Image,
    Video { trim_start: f64 },
}

/// Encoder selection + quality flags, identical for every encoded segment so
/// the concat demuxer can stream-copy them losslessly back together.
fn encoder_args(spec: &EncodeSpec) -> Vec<String> {
    match spec.encoder {
        Encoder::Libx264 => vec![
            String::from("-c:v"),
            String::from("libx264"),
            String::from("-preset"),
            spec.preset.clone(),
            String::from("-crf"),
            spec.crf.to_string(),
        ],
        Encoder::Nvenc => vec![
            String::from("-c:v"),
            String::from("h264_nvenc"),
            String::from("-preset"),
            String::from(match spec.crf {
                0..=18 => "p6",
                19..=23 => "p5",
                _ => "p4",
            }),
            String::from("-rc"),
            String::from("constqp"),
            String::from("-cq"),
            spec.crf.to_string(),
        ],
        Encoder::Amf => vec![
            String::from("-c:v"),
            String::from("h264_amf"),
            String::from("-quality"),
            String::from(match spec.crf {
                0..=18 => "quality",
                19..=23 => "balanced",
                _ => "speed",
            }),
            String::from("-rc"),
            String::from("cqp"),
            String::from("-qp_i"),
            spec.crf.to_string(),
            String::from("-qp_p"),
            spec.crf.to_string(),
        ],
        Encoder::VideoToolbox => vec![
            String::from("-c:v"),
            String::from("h264_videotoolbox"),
            // VideoToolbox takes a 1-99 quality dial instead of CRF.
            String::from("-q:v"),
            ((51 - spec.crf) * 2).clamp(1, 99).to_string(),
        ],
        Encoder::Qsv => vec![
            String::from("-c:v"),
            String::from("h264_qsv"),
            String::from("-rc"),
            String::from("cqp"),
            String::from("-global_quality"),
            spec.crf.to_string(),
        ],
        Encoder::Vaapi => vec![
            String::from("-c:v"),
            String::from("h264_vaapi"),
            String::from("-rc"),
            String::from("cqp"),
            String::from("-global_quality"),
            spec.crf.to_string(),
        ],
    }
}

/// `<ffmpeg> -y ... -i <media> ... -f mpegts <out>`: one timeline item encoded to a
/// uniform silent segment. Input seeking (`-ss` before `-i`) plus transcoding gives
/// accurate cut points; output rate forced so every segment is codec-identical.
pub fn segment_command(
    media: &Path,
    out: &Path,
    spec: &EncodeSpec,
    kind: SegmentKind,
    duration: f64,
) -> Vec<String> {
    let mut args = vec![
        String::from("-y"),
        String::from("-hide_banner"),
        String::from("-loglevel"),
        String::from("error"),
    ];
    match kind {
        SegmentKind::Image => {
            // No -loop 1: that re-decodes and re-scales the source image for
            // every output frame. One decoded frame is cloned by the tpad
            // filter below instead.
        }
        SegmentKind::Video { trim_start } => {
            args.extend([
                String::from("-ss"),
                format_seconds(trim_start),
                String::from("-t"),
                format_seconds(duration),
            ]);
        }
    }
    args.extend([String::from("-i"), media.display().to_string()]);
    match kind {
        SegmentKind::Image if spec.kenburns => {
            // zoompan generates every output frame itself (`d` frames at the
            // segment fps), so no tpad clone is needed — only the -t output cap.
            let frames = (duration * f64::from(spec.fps)).round().max(1.0) as i64;
            match kenburns_filter(spec, frames) {
                FilterPlan::Simple(chain) => {
                    args.extend([String::from("-vf"), chain]);
                }
                FilterPlan::Complex(graph) => {
                    args.extend([
                        String::from("-filter_complex"),
                        graph,
                        String::from("-map"),
                        String::from("[vout]"),
                    ]);
                }
            }
        }
        _ => match segment_filter(spec) {
            FilterPlan::Simple(mut chain) => {
                if matches!(kind, SegmentKind::Image) {
                    chain.push_str(&format!(
                        ",tpad=stop_mode=clone:stop_duration={}",
                        format_seconds(duration)
                    ));
                }
                args.extend([String::from("-vf"), chain]);
            }
            FilterPlan::Complex(mut graph) => {
                if matches!(kind, SegmentKind::Image) {
                    if let Some(at) = graph.rfind("[vout]") {
                        graph.insert_str(
                            at,
                            &format!(
                                ",tpad=stop_mode=clone:stop_duration={}",
                                format_seconds(duration)
                            ),
                        );
                    }
                }
                args.extend([
                    String::from("-filter_complex"),
                    graph,
                    String::from("-map"),
                    String::from("[vout]"),
                ]);
            }
        },
    }
    args.extend([String::from("-r"), spec.fps.to_string()]);
    args.extend(encoder_args(spec));
    if !spec.encoder.needs_hwupload() {
        // JPEG photos decode as full-range (yuvj420p); TVs and most players
        // expect standard limited range, so every segment is forced to tv
        // range on the yuv420p path. QSV/VAAPI consume nv12 hardware frames
        // from hwupload, where forcing yuv420p would conflict.
        args.extend([String::from("-pix_fmt"), String::from("yuv420p")]);
    }
    args.extend([
        String::from("-color_range"),
        String::from("tv"),
        String::from("-an"),
        String::from("-t"),
        format_seconds(duration),
        String::from("-f"),
        String::from("mpegts"),
        out.display().to_string(),
    ]);
    args
}

/// Seconds of overlap each adjacent pair of items shares when a transition is
/// on (clamped further by item durations in the export planner).
pub const TRANSITION_SECONDS: f64 = 0.75;

/// Normalizing prefix for input `idx` of a multi-input filter graph: the exact
/// scale/pad chain `segment_filter` uses so every blend input lands at target
/// WxH, SAR 1 — identical pixel geometry on both sides of an xfade. `tail` is
/// appended verbatim; `label` names the output pad.
fn normalize_fragment(spec: &EncodeSpec, idx: usize, fit: Fit, tail: &str, label: &str) -> String {
    let w = spec.width;
    let h = spec.height;
    match (fit, spec.background) {
        (Fit::Cover, _) => format!(
            "[{idx}:v]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h}{tail}[{label}]"
        ),
        (Fit::Contain, Background::Blur) => format!(
            "[{idx}:v]split=2[fg{idx}][bg{idx}];\
             [bg{idx}]scale={w}:{h}:force_original_aspect_ratio=increase,crop={w}:{h},gblur=sigma=20[bgb{idx}];\
             [fg{idx}]scale={w}:{h}:force_original_aspect_ratio=decrease[fgs{idx}];\
             [bgb{idx}][fgs{idx}]overlay=(W-w)/2:(H-h)/2{tail}[{label}]"
        ),
        (Fit::Contain, background) => format!(
            "[{idx}:v]scale={w}:{h}:force_original_aspect_ratio=decrease,\
             pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:color={}{tail}[{label}]",
            background_color(background)
        ),
    }
}

fn upload_tail(spec: &EncodeSpec) -> &'static str {
    if spec.encoder.needs_hwupload() {
        ",format=nv12,hwupload"
    } else {
        ",format=yuv420p"
    }
}

fn image_loop(x: &str) -> String {
    format!(",tpad=stop_mode=clone:stop_duration={x}")
}

/// `<ffmpeg> -y ... -i a -i b -filter_complex ... -f mpegts <out>`: the x-second
/// blend between item a's tail and item b's head, nothing else. Videos are cut
/// to the exact window with input seeking (a: `[a_trim + a_dur − x, +x]`,
/// b: `[b_trim, +x]`); images loop one decoded frame over the window. xfade
/// starts the transition at offset 0, so the whole output is the blend, and
/// `-t x` caps any clone rounding on the image sides. Encoder flags are
/// byte-identical to `segment_command`, keeping the segment concat-compatible.
pub fn transition_command(
    a_media: &Path,
    a_trim: f64,
    a_dur: f64,
    b_media: &Path,
    b_trim: f64,
    x: f64,
    out: &Path,
    spec: &EncodeSpec,
    transition: &str,
    a_kind: SegmentKind,
    b_kind: SegmentKind,
    a_fit: Fit,
    b_fit: Fit,
) -> Vec<String> {
    let xs = format_seconds(x);
    let mut args = vec![
        String::from("-y"),
        String::from("-hide_banner"),
        String::from("-loglevel"),
        String::from("error"),
    ];
    if let SegmentKind::Video { .. } = a_kind {
        args.extend([
            String::from("-ss"),
            format_seconds(a_trim + a_dur - x),
            String::from("-t"),
            xs.clone(),
        ]);
    }
    args.extend([String::from("-i"), a_media.display().to_string()]);
    if let SegmentKind::Video { .. } = b_kind {
        args.extend([String::from("-ss"), format_seconds(b_trim), String::from("-t"), xs.clone()]);
    }
    args.extend([String::from("-i"), b_media.display().to_string()]);

    let sync = format!(",setsar=1,fps={}", spec.fps);
    let a_image = matches!(a_kind, SegmentKind::Image);
    let b_image = matches!(b_kind, SegmentKind::Image);
    let graph = if transition == "checkerboard" {
        // True checkerboard: a gray mask whose cells (64 px squares) reveal in
        // a diagonal wavefront — cell c's alpha ramps clamp((T/x)·C − c, 0, 1)
        // for c = floor(X/64) + floor(Y/64) over C diagonals, so the sweep is
        // monotone and fully revealed exactly at T = x. b is alpha-merged with
        // that mask and overlaid on a.
        let cells = (spec.width + 63) / 64 + (spec.height + 63) / 64 - 1;
        let expr = format!(
            "255*clip((T/{xs})*{cells}-(floor(X/64)+floor(Y/64)),0,1)"
        );
        format!(
            "{};{};nullsrc=s={}x{}:r={}:d={xs},geq=lum='{}':cb=128:cr=128,format=gray[mask];\
             [nb][mask]alphamerge[bm];[na][bm]overlay=0:0[vx];[vx]setsar=1{}[vout]",
            normalize_fragment(
                spec,
                0,
                a_fit,
                &format!("{sync},format=yuv420p{}", if a_image { image_loop(&xs) } else { String::new() }),
                "na",
            ),
            normalize_fragment(
                spec,
                1,
                b_fit,
                &format!("{sync},format=yuva420p{}", if b_image { image_loop(&xs) } else { String::new() }),
                "nb",
            ),
            spec.width,
            spec.height,
            spec.fps,
            expr,
            upload_tail(spec),
        )
    } else {
        let name = if transition == "slide" { "slideleft" } else { "fade" };
        format!(
            "{};{};[na][nb]xfade=transition={name}:duration={xs}:offset=0[vx];\
             [vx]setsar=1,fps={}{}[vout]",
            normalize_fragment(
                spec,
                0,
                a_fit,
                &format!("{sync},format=yuv420p{}", if a_image { image_loop(&xs) } else { String::new() }),
                "na",
            ),
            normalize_fragment(
                spec,
                1,
                b_fit,
                &format!("{sync},format=yuv420p{}", if b_image { image_loop(&xs) } else { String::new() }),
                "nb",
            ),
            spec.fps,
            upload_tail(spec),
        )
    };
    args.extend([
        String::from("-filter_complex"),
        graph,
        String::from("-map"),
        String::from("[vout]"),
    ]);
    args.extend([String::from("-r"), spec.fps.to_string()]);
    args.extend(encoder_args(spec));
    if !spec.encoder.needs_hwupload() {
        args.extend([String::from("-pix_fmt"), String::from("yuv420p")]);
    }
    args.extend([
        String::from("-color_range"),
        String::from("tv"),
        String::from("-an"),
        String::from("-t"),
        xs,
        String::from("-f"),
        String::from("mpegts"),
        out.display().to_string(),
    ]);
    args
}

pub fn thumbnail_command(media: &Path, out: &Path, seek: Option<f64>) -> Vec<String> {
    let mut args = vec![
        String::from("-y"),
        String::from("-hide_banner"),
        String::from("-loglevel"),
        String::from("error"),
    ];
    if let Some(seek) = seek {
        args.extend([String::from("-ss"), format_seconds(seek)]);
    }
    args.extend([
        String::from("-i"),
        media.display().to_string(),
        String::from("-vf"),
        String::from("scale=480:-2,setsar=1,format=yuvj420p"),
        String::from("-frames:v"),
        String::from("1"),
        String::from("-q:v"),
        String::from("5"),
        out.display().to_string(),
    ]);
    args
}

pub fn probe_command(media: &Path) -> Vec<String> {
    vec![
        String::from("-v"),
        String::from("error"),
        String::from("-print_format"),
        String::from("json"),
        String::from("-show_format"),
        String::from("-show_streams"),
        media.display().to_string(),
    ]
}

pub fn concat_command(list: &Path, out: &Path) -> Vec<String> {
    vec![
        String::from("-y"),
        String::from("-hide_banner"),
        String::from("-loglevel"),
        String::from("error"),
        String::from("-f"),
        String::from("concat"),
        String::from("-safe"),
        String::from("0"),
        String::from("-i"),
        list.display().to_string(),
        String::from("-c"),
        String::from("copy"),
        String::from("-f"),
        String::from("mpegts"),
        out.display().to_string(),
    ]
}

pub fn remux_command(merged: &Path, out: &Path) -> Vec<String> {
    vec![
        String::from("-y"),
        String::from("-hide_banner"),
        String::from("-loglevel"),
        String::from("error"),
        String::from("-i"),
        merged.display().to_string(),
        String::from("-c"),
        String::from("copy"),
        String::from("-movflags"),
        String::from("+faststart"),
        out.display().to_string(),
    ]
}

/// `[1:a]volume..,afade in..,afade out..[a]` — fades skipped when zero or when the
/// whole slideshow is shorter than the fade itself.
pub fn audio_filter(total_duration: f64, volume: f64, fade_in: f64, fade_out: f64) -> String {
    let mut chain = format!("volume={:.2}", volume);
    if fade_in > 0.0 {
        chain.push_str(&format!(",afade=t=in:st=0:d={}", format_seconds(fade_in)));
    }
    if fade_out > 0.0 && total_duration > fade_out {
        chain.push_str(&format!(
            ",afade=t=out:st={}:d={}",
            format_seconds(total_duration - fade_out),
            format_seconds(fade_out)
        ));
    }
    format!("[1:a]{chain}[a]", chain = chain)
}

/// Video copied losslessly; music looped if shorter than the slideshow and trimmed
/// by `-shortest`; `+faststart` so phones/TVs start playback before the file ends.
pub fn mux_command(
    merged: &Path,
    song: &Path,
    out: &Path,
    total_duration: f64,
    volume: f64,
    fade_in: f64,
    fade_out: f64,
    loop_track: bool,
) -> Vec<String> {
    let mut args = vec![
        String::from("-y"),
        String::from("-hide_banner"),
        String::from("-loglevel"),
        String::from("error"),
        String::from("-i"),
        merged.display().to_string(),
    ];
    if loop_track {
        args.extend([String::from("-stream_loop"), String::from("-1")]);
    }
    args.extend([
        String::from("-i"),
        song.display().to_string(),
        String::from("-filter_complex"),
        audio_filter(total_duration, volume, fade_in, fade_out),
        String::from("-map"),
        String::from("0:v"),
        String::from("-map"),
        String::from("[a]"),
        String::from("-c:v"),
        String::from("copy"),
        String::from("-c:a"),
        String::from("aac"),
        String::from("-b:a"),
        String::from("192k"),
        String::from("-shortest"),
        String::from("-movflags"),
        String::from("+faststart"),
        out.display().to_string(),
    ]);
    args
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn spec() -> EncodeSpec {
        EncodeSpec {
            width: 1920,
            height: 1080,
            fps: 30,
            crf: 21,
            preset: "medium".to_string(),
            encoder: Encoder::Libx264,
            fit: Fit::Contain,
            background: Background::Black,
            kenburns: false,
        }
    }

    #[test]
    fn gpu_encoders_get_encoder_specific_flags() {
        let media = Path::new("photo.jpg");
        let out = Path::new("out.ts");

        let nvenc = {
            let mut s = spec();
            s.encoder = Encoder::Nvenc;
            segment_command(media, out, &s, SegmentKind::Image, 3.0).join(" ")
        };
        assert!(nvenc.contains("-c:v h264_nvenc -preset p5 -rc constqp -cq 21"));
        assert!(nvenc.contains("-pix_fmt yuv420p"));
        assert!(!nvenc.contains("libx264"));

        let amf = {
            let mut s = spec();
            s.encoder = Encoder::Amf;
            s.crf = 24;
            segment_command(media, out, &s, SegmentKind::Image, 3.0).join(" ")
        };
        assert!(amf.contains("-c:v h264_amf -quality speed -rc cqp -qp_i 24 -qp_p 24"));

        let toolbox = {
            let mut s = spec();
            s.encoder = Encoder::VideoToolbox;
            segment_command(media, out, &s, SegmentKind::Image, 3.0).join(" ")
        };
        assert!(toolbox.contains("-c:v h264_videotoolbox -q:v 60"));

        // Quick Sync and VAAPI consume uploaded hardware frames instead of
        // plain yuv420p.
        let qsv = {
            let mut s = spec();
            s.encoder = Encoder::Qsv;
            segment_command(media, out, &s, SegmentKind::Image, 3.0).join(" ")
        };
        assert!(qsv.contains("fps=30,format=nv12,hwupload"));
        assert!(qsv.contains("-c:v h264_qsv -rc cqp -global_quality 21"));
        assert!(!qsv.contains("-pix_fmt"));

        let vaapi = {
            let mut s = spec();
            s.encoder = Encoder::Vaapi;
            s.fit = Fit::Cover;
            segment_command(media, out, &s, SegmentKind::Image, 3.0).join(" ")
        };
        assert!(vaapi.contains("crop=1920:1080,setsar=1,fps=30,format=nv12,hwupload"));
        assert!(vaapi.contains("-c:v h264_vaapi -rc cqp -global_quality 21"));
    }

    #[test]
    fn gpu_preference_falls_back_when_encoder_unavailable() {
        let bins = Binaries {
            ffmpeg: PathBuf::from("/nonexistent/ffmpeg"),
            ffprobe: PathBuf::from("/nonexistent/ffprobe"),
        };
        assert_eq!(resolve_encoder(&bins, "gpu"), Encoder::Libx264);
        assert_eq!(resolve_encoder(&bins, "auto"), Encoder::Libx264);
        assert_eq!(resolve_encoder(&bins, "cpu"), Encoder::Libx264);
    }

    #[test]
    fn parses_ffmpeg_version_banner() {
        assert_eq!(
            parse_version_line(
                "ffmpeg version n9.0.2 Copyright (c) 2000-2026 the FFmpeg developers"
            ),
            Some("n9.0.2".to_string())
        );
        assert_eq!(parse_version_line("not ffmpeg at all"), None);
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn sidecar_name_matches_target_triple_convention() {
        assert_eq!(tool_file_name("ffmpeg"), "ffmpeg-x86_64-unknown-linux-gnu");
        assert_eq!(tool_file_name("ffprobe"), "ffprobe-x86_64-unknown-linux-gnu");
    }

    #[test]
    fn image_segment_command_letterboxes_to_black() {
        let args = segment_command(
            Path::new("/p/media/a-photo.jpg"),
            Path::new("/p/cache/seg.ts"),
            &spec(),
            SegmentKind::Image,
            3.0,
        );
        let joined = args.join(" ");
        assert!(joined.contains("-i /p/media/a-photo.jpg"));
        assert!(!joined.contains("-loop"));
        assert!(joined.contains("pad=1920:1080:(ow-iw)/2:(oh-ih)/2:color=black"));
        assert!(joined.contains("tpad=stop_mode=clone:stop_duration=3.000"));
        assert!(joined.contains("setsar=1,fps=30,format=yuv420p"));
        assert!(joined.contains("-an -t 3.000 -f mpegts"));
        assert!(args.last().unwrap().ends_with("seg.ts"));
        assert!(!joined.contains("-filter_complex"));
    }

    #[test]
    fn video_segment_command_seeks_before_input_and_mutes() {
        let args = segment_command(
            Path::new("/p/media/v.MOV"),
            Path::new("/p/cache/seg2.ts"),
            &spec(),
            SegmentKind::Video { trim_start: 2.5 },
            10.4,
        );
        let joined = args.join(" ");
        assert!(joined.contains("-ss 2.500 -t 10.400 -i /p/media/v.MOV"));
        assert!(joined.contains("-an"));
    }

    #[test]
    fn cover_fit_crops_instead_of_padding() {
        let mut s = spec();
        s.fit = Fit::Cover;
        match segment_filter(&s) {
            FilterPlan::Simple(chain) => {
                assert!(chain.contains("force_original_aspect_ratio=increase,crop=1920:1080"));
                assert!(!chain.contains("pad="));
            }
            FilterPlan::Complex(_) => panic!("cover fit must be a simple chain"),
        }
    }

    #[test]
    fn blur_background_builds_overlay_graph() {
        let mut s = spec();
        s.background = Background::Blur;
        match segment_filter(&s) {
            FilterPlan::Complex(graph) => {
                assert!(graph.starts_with("[0:v]split=2[fg][bg];"));
                assert!(graph.contains("gblur=sigma=20[bgb]"));
                assert!(graph.contains("overlay=(W-w)/2:(H-h)/2"));
                assert!(graph.ends_with("[vout]"));
            }
            FilterPlan::Simple(_) => panic!("blur must be a complex graph"),
        }
    }

    #[test]
    fn music_fades_land_at_timeline_end() {
        let filter = audio_filter(42.0, 0.8, 1.0, 3.0);
        assert_eq!(
            filter,
            "[1:a]volume=0.80,afade=t=in:st=0:d=1.000,afade=t=out:st=39.000:d=3.000[a]"
        );
    }

    #[test]
    fn fades_are_skipped_when_pointless() {
        // Slideshow shorter than the fade-out: no negative fade start.
        let filter = audio_filter(2.0, 1.0, 0.0, 3.0);
        assert_eq!(filter, "[1:a]volume=1.00[a]");
    }

    #[test]
    fn mux_loops_short_music_and_trims_to_video() {
        let args = mux_command(
            Path::new("/p/cache/merged.ts"),
            Path::new("/p/media/song.mp3"),
            Path::new("/out.mp4"),
            120.0,
            0.8,
            1.0,
            3.0,
            true,
        );
        let joined = args.join(" ");
        assert!(joined.contains("-stream_loop -1 -i /p/media/song.mp3"));
        assert!(joined.contains("-map 0:v -map [a] -c:v copy -c:a aac -b:a 192k -shortest"));
        assert!(joined.contains("-movflags +faststart"));
        // No loop when loop disabled.
        let args = mux_command(
            Path::new("m.ts"),
            Path::new("s.mp3"),
            Path::new("o.mp4"),
            10.0,
            0.8,
            0.0,
            0.0,
            false,
        );
        assert!(!args.iter().any(|a| a == "-stream_loop"));
    }

    #[test]
    fn concat_copies_streams_without_reencoding() {
        let args = concat_command(
            Path::new("/p/cache/list.txt"),
            Path::new("/p/cache/merged.ts"),
        );
        let joined = args.join(" ");
        assert!(joined.contains("-f concat -safe 0 -i /p/cache/list.txt -c copy -f mpegts"));
    }

    #[test]
    fn crossfade_blend_maps_two_loops_through_xfade() {
        let args = transition_command(
            Path::new("/p/media/a.jpg"),
            0.0,
            3.0,
            Path::new("/p/media/b.jpg"),
            0.0,
            TRANSITION_SECONDS,
            Path::new("/p/cache/blend.ts"),
            &spec(),
            "crossfade",
            SegmentKind::Image,
            SegmentKind::Image,
            Fit::Contain,
            Fit::Contain,
        );
        let joined = args.join(" ");
        assert!(joined.contains("-i /p/media/a.jpg -i /p/media/b.jpg"), "{joined}");
        // Images: no input seeking, each side loops its single decoded frame.
        assert!(!joined.contains("-ss"));
        assert_eq!(joined.matches("tpad=stop_mode=clone:stop_duration=0.750").count(), 2);
        assert!(joined.contains("xfade=transition=fade:duration=0.750:offset=0[vx]"));
        assert!(joined.contains("-filter_complex") && joined.contains("-map [vout]"));
        // Encoder args identical to segment_command for the same spec.
        assert!(joined.contains("-c:v libx264 -preset medium -crf 21"));
        assert!(joined.contains("-pix_fmt yuv420p -color_range tv -an -t 0.750 -f mpegts"));
        assert!(args.last().unwrap().ends_with("blend.ts"));
    }

    #[test]
    fn slide_blend_cuts_video_windows_around_the_cut() {
        let args = transition_command(
            Path::new("/p/media/a.mp4"),
            0.5,
            2.5,
            Path::new("/p/media/b.mp4"),
            1.5,
            TRANSITION_SECONDS,
            Path::new("/p/cache/blend.ts"),
            &spec(),
            "slide",
            SegmentKind::Video { trim_start: 0.5 },
            SegmentKind::Video { trim_start: 1.5 },
            Fit::Contain,
            Fit::Cover,
        );
        let joined = args.join(" ");
        // a's tail window [0.5 + 2.5 − 0.75, +0.75]; b's head window [1.5, +0.75].
        assert!(joined.contains("-ss 2.250 -t 0.750 -i /p/media/a.mp4"), "{joined}");
        assert!(joined.contains("-ss 1.500 -t 0.750 -i /p/media/b.mp4"), "{joined}");
        assert!(joined.contains("xfade=transition=slideleft:duration=0.750:offset=0[vx]"));
        // Video sides loop nothing.
        assert!(!joined.contains("tpad"));
        assert!(joined.contains("crop=1920:1080")); // b normalizes with its own cover fit
    }

    #[test]
    fn checkerboard_blend_builds_a_sweeping_cell_mask() {
        let args = transition_command(
            Path::new("/p/media/a.jpg"),
            0.0,
            3.0,
            Path::new("/p/media/b.jpg"),
            0.0,
            TRANSITION_SECONDS,
            Path::new("/p/cache/blend.ts"),
            &spec(),
            "checkerboard",
            SegmentKind::Image,
            SegmentKind::Image,
            Fit::Contain,
            Fit::Contain,
        );
        let joined = args.join(" ");
        // 1920x1080 => 30 columns + 17 rows => 46 diagonals; diagonal sweep.
        assert!(joined.contains(
            "geq=lum='255*clip((T/0.750)*46-(floor(X/64)+floor(Y/64)),0,1)':cb=128:cr=128"
        ), "{joined}");
        assert!(joined.contains("[nb][mask]alphamerge[bm];[na][bm]overlay=0:0[vx]"));
        assert!(!joined.contains("xfade"));
        assert!(joined.contains("-an"));
        assert!(joined.contains("-c:v libx264 -preset medium -crf 21"));
        assert!(joined.contains("-f mpegts"));
    }

    #[test]
    fn kenburns_images_zoompan_exact_frame_counts() {
        let mut s = spec();
        s.kenburns = true;
        let joined = segment_command(
            Path::new("/p/media/a.jpg"),
            Path::new("/p/cache/seg.ts"),
            &s,
            SegmentKind::Image,
            3.0,
        )
        .join(" ");
        // d = round(3 s * 30 fps); step makes zoom land on 1.18 at the last frame.
        assert!(joined.contains("zoompan=z='min(zoom+0.002000,1.18)':d=90"), "{joined}");
        assert!(joined.contains("s=1920x1080:fps=30"));
        // 2x target first so the zoom window stays sharper than the output.
        assert!(joined.contains("scale=3840:2160:force_original_aspect_ratio=decrease,pad=3840:2160:(ow-iw)/2:(oh-ih)/2:color=black"));
        assert!(!joined.contains("tpad"));
        assert!(joined.contains("-an -t 3.000 -f mpegts"));

        // Videos are unaffected by Ken Burns.
        let video = segment_command(
            Path::new("/p/media/v.mp4"),
            Path::new("/p/cache/seg2.ts"),
            &s,
            SegmentKind::Video { trim_start: 1.0 },
            4.0,
        )
        .join(" ");
        assert!(!video.contains("zoompan"));
        assert!(video.contains("-ss 1.000 -t 4.000 -i /p/media/v.mp4"));
    }
}
