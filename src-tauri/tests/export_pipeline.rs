//! End-to-end pipeline proof: synthetic media -> import -> export -> ffprobe.
//! Requires an ffmpeg engine (bundled sidecar, LANTERN_FFMPEG, or PATH);
//! skips with a message when none exists so unit-only environments still pass.

use std::path::{Path, PathBuf};
use std::process::Command as StdCommand;
use std::sync::Arc;

use lantern_lib::{
    export,
    ffmpeg::Binaries,
    media,
    project::{AssetKind, Background, Fit, ProjectCtx},
    state::ExportCtl,
};

fn engine() -> Option<Binaries> {
    let bins = Binaries::resolve(None);
    let available = StdCommand::new(&bins.ffmpeg)
        .arg("-version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if available {
        Some(bins)
    } else {
        None
    }
}

fn workdir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lantern-test-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("temp dir");
    dir
}

fn run_ffmpeg(bins: &Binaries, args: &[&str]) {
    let output = StdCommand::new(&bins.ffmpeg)
        .args(args)
        .output()
        .expect("ffmpeg spawn");
    assert!(
        output.status.success(),
        "ffmpeg {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn generate_inputs(bins: &Binaries, dir: &Path) {
    run_ffmpeg(
        bins,
        &[
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "testsrc2=size=1920x1080:rate=30:duration=1",
            "-frames:v", "1",
            &dir.join("beach 2019.jpg").display().to_string(),
        ],
    );
    run_ffmpeg(
        bins,
        &[
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "color=c=0x3a7ca8:size=640x480:duration=1",
            "-frames:v", "1",
            &dir.join("portrait.png").display().to_string(),
        ],
    );
    run_ffmpeg(
        bins,
        &[
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "testsrc=size=640x480:rate=30:duration=2.5",
            "-c:v", "libx264", "-pix_fmt", "yuv420p",
            &dir.join("family-clip.mp4").display().to_string(),
        ],
    );
    run_ffmpeg(
        bins,
        &[
            "-y", "-hide_banner", "-loglevel", "error",
            "-f", "lavfi", "-i", "sine=frequency=440:duration=5",
            &dir.join("song.wav").display().to_string(),
        ],
    );
}

fn ffprobe_text(bins: &Binaries, file: &Path, args: &[&str]) -> String {
    let output = StdCommand::new(&bins.ffprobe)
        .arg("-v")
        .arg("error")
        .args(args)
        .arg(file)
        .output()
        .expect("ffprobe spawn");
    assert!(output.status.success());
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

#[tokio::test]
async fn full_pipeline_produces_playable_mp4() {
    let Some(bins) = engine() else {
        eprintln!("skipping export_pipeline test: no ffmpeg engine available");
        return;
    };

    let dir = workdir("e2e");
    generate_inputs(&bins, &dir);

    let mut ctx = ProjectCtx::create(&dir, "Test Show").expect("create project");
    ctx.project.settings.preset = "veryfast".to_string();
    ctx.project.settings.crf = 30;

    let files: Vec<String> = [
        "beach 2019.jpg",
        "portrait.png",
        "family-clip.mp4",
        "song.wav",
    ]
    .iter()
    .map(|name| dir.join(name).display().to_string())
    .collect();
    let outcome = media::import(&bins, &ctx.dir, &mut ctx.project, &files).await;
    assert!(outcome.skipped.is_empty(), "unexpected skips: {outcome:?}");
    assert_eq!(outcome.added.len(), 4);
    assert_eq!(ctx.project.items.len(), 3);
    assert!(
        ctx.project.audio.is_some(),
        "a song dropped with the photos becomes the soundtrack"
    );

    let images = outcome
        .added
        .iter()
        .filter(|asset| asset.kind == AssetKind::Image)
        .count();
    let videos = outcome
        .added
        .iter()
        .filter(|asset| asset.kind == AssetKind::Video)
        .count();
    assert_eq!((images, videos), (2, 1));
    let clip = outcome
        .added
        .iter()
        .find(|asset| asset.kind == AssetKind::Video)
        .expect("video asset");
    assert!(clip.duration.unwrap_or(0.0) > 2.0 && clip.duration.unwrap_or(9.0) < 3.0);

    // Replacing the song via the music-card path works too.
    media::import_song(
        &bins,
        &ctx.dir,
        &mut ctx.project,
        &dir.join("song.wav").display().to_string(),
    )
    .await
    .expect("import song");
    assert!(ctx.project.audio.is_some());
    ctx.save().expect("save");

    // Polish coverage: blurred letterbox background globally, a cover-cropped
    // photo, and a video trimmed to a 1.5s slice starting at 0.5s.
    ctx.project.settings.background = Background::Blur;
    let portrait_id = ctx
        .project
        .assets
        .values()
        .find(|asset| asset.file_name.ends_with("portrait.png"))
        .expect("portrait asset")
        .id
        .clone();
    let video_id = ctx
        .project
        .assets
        .values()
        .find(|asset| asset.kind == AssetKind::Video)
        .expect("video asset")
        .id
        .clone();
    for item in &mut ctx.project.items {
        if item.asset_id == portrait_id {
            item.fit = Fit::Cover;
        }
        if item.asset_id == video_id {
            item.trim_start = 0.5;
            item.duration = Some(1.5);
        }
    }
    ctx.save().expect("save edits");

    // Export 1: everything fresh.
    let ctl = Arc::new(ExportCtl::new());
    ctl.begin().expect("begin");
    let out_path = dir.join("out").join("show.mp4");
    let mut events: Vec<(String, usize, usize)> = Vec::new();
    let result = export::export_project(
        &bins,
        &ctx,
        &out_path,
        &ctl,
        &mut |event| events.push((event.phase.clone(), event.index, event.total)),
    )
    .await;
    ctl.end();
    let summary = match result {
        Ok(summary) => summary,
        Err(e) => panic!("export failed: {e}"),
    };

    // Human-readable progress: three prepares, a stitch, a music pass.
    assert_eq!(events.iter().filter(|(phase, ..)| phase == "preparing").count(), 3);
    assert!(events.iter().any(|(phase, ..)| phase == "stitching"));
    assert!(events.iter().any(|(phase, ..)| phase == "music"));

    // 3s + 3s photos + 1.5s trimmed video slice.
    assert!(
        (summary.total_duration - 7.5).abs() < 0.2,
        "timeline duration drifted: {}",
        summary.total_duration
    );

    let out_path = PathBuf::from(&summary.out_path);
    assert!(out_path.exists());

    let codecs = ffprobe_text(
        &bins,
        &out_path,
        &["-show_entries", "stream=codec_name", "-of", "csv=p=0"],
    );
    let codecs: Vec<&str> = codecs.lines().map(str::trim).collect();
    assert!(codecs.contains(&"h264"), "codecs: {codecs:?}");
    assert!(codecs.contains(&"aac"), "codecs: {codecs:?}");

    let duration: f64 = ffprobe_text(
        &bins,
        &out_path,
        &["-show_entries", "format=duration", "-of", "default=nk=1:nw=1"],
    )
    .trim()
    .parse()
    .expect("duration parse");
    assert!(
        (duration - 7.5).abs() < 0.3,
        "output duration mismatch: {duration}"
    );

    let pixel_format = ffprobe_text(
        &bins,
        &out_path,
        &["-show_entries", "stream=codec_name,pix_fmt,avg_frame_rate", "-of", "csv=p=0"],
    );
    assert!(pixel_format.contains("yuv420p"), "{pixel_format}");
    assert!(
        pixel_format.contains("30/1"),
        "expected constant 30 fps: {pixel_format}"
    );

    // +faststart: moov must precede mdat so playback starts before download ends.
    let bytes = std::fs::read(&out_path).expect("read output");
    fn find_pattern(hay: &[u8], needle: &[u8]) -> Option<usize> {
        hay.windows(needle.len()).position(|w| w == needle)
    }
    let moov = find_pattern(&bytes, b"moov").expect("moov atom");
    let mdat = find_pattern(&bytes, b"mdat").expect("mdat atom");
    assert!(moov < mdat, "faststart violated: moov at {moov}, mdat at {mdat}");

    // Export 2: segments are cached; not a single prepare stage re-runs.
    let segment_mtimes: Vec<std::time::SystemTime> = std::fs::read_dir(dir.join("Test Show.lantern/cache"))
        .expect("cache dir")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.file_name().to_string_lossy().ends_with(".ts").then(|| {
                entry.metadata().ok().and_then(|m| m.modified().ok())
            })?
        })
        .collect();
    assert_eq!(segment_mtimes.len(), 3, "expected three cached segments");
    ctl.begin().expect("begin 2");
    let out2 = dir.join("out").join("show2.mp4");
    let mut second_events: Vec<(String, usize, usize)> = Vec::new();
    let result2 = export::export_project(
        &bins,
        &ctx,
        &out2,
        &ctl,
        &mut |event| second_events.push((event.phase.clone(), event.index, event.total)),
    )
    .await;
    ctl.end();
    assert!(result2.is_ok(), "re-export failed: {:?}", result2.err());
    // Reused pieces are credited with a single progress tick; nothing re-encodes.
    assert_eq!(
        second_events.iter().filter(|(phase, ..)| phase == "preparing").count(),
        1,
        "cached export should emit one reuse tick"
    );
    let segment_mtimes_after: Vec<std::time::SystemTime> = std::fs::read_dir(dir.join("Test Show.lantern/cache"))
        .expect("cache dir")
        .filter_map(|entry| {
            let entry = entry.ok()?;
            entry.file_name().to_string_lossy().ends_with(".ts").then(|| {
                entry.metadata().ok().and_then(|m| m.modified().ok())
            })?
        })
        .collect();
    assert_eq!(
        segment_mtimes, segment_mtimes_after,
        "cached segments must not be re-encoded"
    );
    assert!(out2.exists());

    let _ = std::fs::remove_dir_all(&dir);
}
