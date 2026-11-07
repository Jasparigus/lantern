use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use std::process::Stdio;
use tokio::process::Command;
use uuid::Uuid;
use zune_heic::HeifDecoder;

use crate::ffmpeg::{self, Binaries};
use crate::project::{Asset, AssetKind, AudioSettings, Item, Project, MEDIA_DIR, THUMBS_DIR};

const IMAGE_EXTS: &[&str] = &["jpg", "jpeg", "png", "webp", "bmp", "tif", "tiff", "gif"];
const HEIC_EXTS: &[&str] = &["heic", "heif"];
const VIDEO_EXTS: &[&str] = &[
    "mp4", "mov", "m4v", "mkv", "webm", "avi", "mts", "m2ts", "wmv", "flv", "3gp",
];
const AUDIO_EXTS: &[&str] = &[
    "mp3", "m4a", "aac", "wav", "ogg", "oga", "flac", "opus", "wma",
];

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SkippedFile {
    pub path: String,
    pub reason: String,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ImportOutcome {
    pub added: Vec<Asset>,
    pub skipped: Vec<SkippedFile>,
}

fn extension_of(path: &Path) -> String {
    path.extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

fn ext_category(ext: &str) -> Option<&'static str> {
    if IMAGE_EXTS.contains(&ext) || HEIC_EXTS.contains(&ext) {
        Some("image")
    } else if VIDEO_EXTS.contains(&ext) {
        Some("video")
    } else if AUDIO_EXTS.contains(&ext) {
        Some("audio")
    } else {
        None
    }
}

fn sanitize_stem(stem: &str) -> String {
    let cleaned: String = stem
        .chars()
        .map(|c| {
            if matches!(c, '/' | '\\' | ':' | '\0' | '?' | '*' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                c
            }
        })
        .collect();
    let truncated: String = cleaned.chars().take(80).collect();
    if truncated.trim().is_empty() {
        "photo".to_string()
    } else {
        truncated
    }
}

#[derive(Deserialize, Default)]
struct ProbeFormat {
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Deserialize, Default)]
struct ProbeStream {
    #[serde(default)]
    codec_type: Option<String>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    duration: Option<String>,
}

#[derive(Deserialize, Default)]
struct ProbeJson {
    #[serde(default)]
    format: Option<ProbeFormat>,
    #[serde(default)]
    streams: Option<Vec<ProbeStream>>,
}

pub struct ProbeInfo {
    pub duration: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub has_video: bool,
    pub has_audio: bool,
}

fn parse_duration(value: Option<&String>) -> Option<f64> {
    value.and_then(|v| v.parse::<f64>().ok())
}

async fn probe(bins: &Binaries, path: &Path) -> Result<ProbeInfo, String> {
    let output = Command::new(&bins.ffprobe)
        .args(ffmpeg::probe_command(path))
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| "This file couldn't be read (it may be damaged).".to_string())?;
    if !output.status.success() {
        return Err(
            "This file couldn't be read. It may not be a photo, video, or song.".to_string(),
        );
    }
    let json: ProbeJson = serde_json::from_slice(&output.stdout)
        .map_err(|_| "This file couldn't be understood.".to_string())?;
    let streams = json.streams.unwrap_or_default();
    let format_duration = parse_duration(json.format.as_ref().and_then(|f| f.duration.as_ref()));
    let stream_duration = streams
        .iter()
        .filter_map(|s| parse_duration(s.duration.as_ref()))
        .reduce(f64::max);
    let duration = [format_duration, stream_duration]
        .into_iter()
        .flatten()
        .reduce(f64::max);
    let first_video = streams.iter().find(|s| s.codec_type.as_deref() == Some("video"));
    Ok(ProbeInfo {
        duration,
        width: first_video.and_then(|s| s.width),
        height: first_video.and_then(|s| s.height),
        has_video: streams
            .iter()
            .any(|s| s.codec_type.as_deref() == Some("video")),
        has_audio: streams
            .iter()
            .any(|s| s.codec_type.as_deref() == Some("audio")),
    })
}

/// Extension hints are not trusted: the probe decides what a file really is.
/// A media stream with no measurable length is a still image.
fn decide_kind(probe: &ProbeInfo) -> Option<AssetKind> {
    if probe.has_video {
        Some(if probe.duration.unwrap_or(0.0) >= 1.0 {
            AssetKind::Video
        } else {
            AssetKind::Image
        })
    } else if probe.has_audio {
        Some(AssetKind::Audio)
    } else {
        None
    }
}

/// HEIC/HEIF (iPhone photos) never touch ffmpeg: none of the redistributable
/// ffmpeg builds include libheif. zune-heic decodes in pure Rust and the
/// converted JPEG then flows through the normal pipeline. zune-heic already
/// applies the file's rotation/mirroring and clean-aperture cropping.
fn convert_heic_to_jpeg(src: &Path, dest: &Path) -> Result<(u32, u32), String> {
    use zune_heic::zune_core::colorspace::ColorSpace;

    let data = std::fs::read(src).map_err(|_| "The photo couldn't be read.".to_string())?;
    let mut decoder = HeifDecoder::new(zune_heic::zune_core::bytestream::ZCursor::new(&data));
    let pixels = decoder.decode().map_err(|e| {
        eprintln!("[heic] decode error for {:?}: {e:?}", src);
        "iPhone photos in this format can't be opened yet — open the photo and save it as a JPEG."
            .to_string()
    })?;
    let width = decoder
        .width()
        .and_then(|w| u32::try_from(w).ok())
        .ok_or("The photo has no readable size.")?;
    let height = decoder
        .height()
        .and_then(|h| u32::try_from(h).ok())
        .ok_or("The photo has no readable size.")?;
    let colorspace = decoder
        .colorspace()
        .ok_or("The photo uses an unusual colour mode.")?;

    let pixels_len_ok = |need: usize| pixels.len() >= need * (width as usize) * (height as usize);

    let rgb: image::RgbImage = match colorspace {
        ColorSpace::RGB if pixels_len_ok(3) => {
            image::RgbImage::from_raw(width, height, pixels)
                .ok_or("The photo's pixels could not be processed.")?
        }
        ColorSpace::RGBA if pixels_len_ok(4) => {
            let mut out = Vec::with_capacity((width as usize) * (height as usize) * 3);
            for px in pixels.chunks_exact(4) {
                out.extend_from_slice(&px[..3]);
            }
            image::RgbImage::from_raw(width, height, out)
                .ok_or("The photo's pixels could not be processed.")?
        }
        ColorSpace::Luma if pixels_len_ok(1) => {
            let mut out = Vec::with_capacity(pixels.len() * 3);
            for &v in &pixels {
                out.extend_from_slice(&[v, v, v]);
            }
            image::RgbImage::from_raw(width, height, out)
                .ok_or("The photo's pixels could not be processed.")?
        }
        ColorSpace::LumaA if pixels_len_ok(2) => {
            let mut out = Vec::with_capacity(pixels.len() / 2 * 3);
            for c in pixels.chunks_exact(2) {
                out.extend_from_slice(&[c[0], c[0], c[0]]);
            }
            image::RgbImage::from_raw(width, height, out)
                .ok_or("The photo's pixels could not be processed.")?
        }
        other => {
            return Err(format!(
                "This photo uses a colour mode ({other:?}) the app can't open yet."
            ));
        }
    };

    let file = std::fs::File::create(dest)
        .map_err(|_| "The slideshow folder could not be written to.".to_string())?;
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(file, 92);
    encoder
        .encode(rgb.as_raw(), width, height, image::ExtendedColorType::Rgb8)
        .map_err(|_| "The converted photo could not be saved.".to_string())?;
    Ok((width, height))
}

async fn make_thumbnail(
    bins: &Binaries,
    media: &Path,
    thumb: &Path,
    kind: AssetKind,
    duration: Option<f64>,
) -> Result<(), String> {
    let seek = match kind {
        AssetKind::Video => Some(duration.unwrap_or(1.0).min(2.0) / 2.0),
        _ => None,
    };
    let output = Command::new(&bins.ffmpeg)
        .args(ffmpeg::thumbnail_command(media, thumb, seek))
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .output()
        .await
        .map_err(|_| "The video engine couldn't run.".to_string())?;
    let too_small = std::fs::metadata(thumb).map(|m| m.len() < 512).unwrap_or(true);
    if !output.status.success() || too_small {
        let _ = std::fs::remove_file(thumb);
        return Err("This file couldn't be processed (it may be damaged).".to_string());
    }
    Ok(())
}

struct Prepared {
    file_name: String,
    kind: AssetKind,
    width: Option<u32>,
    height: Option<u32>,
    duration: Option<f64>,
}

/// Copies files into the project, probes them, builds thumbnails, and appends a
/// timeline item for each photo/video (songs become the audio track asset).
pub async fn import(
    bins: &Binaries,
    project_dir: &Path,
    project: &mut Project,
    paths: &[String],
) -> ImportOutcome {
    let mut added = Vec::new();
    let mut skipped = Vec::new();
    let mut first_song: Option<String> = None;

    if let Err(e) = std::fs::create_dir_all(project_dir.join(MEDIA_DIR))
        .and_then(|_| std::fs::create_dir_all(project_dir.join(THUMBS_DIR)))
    {
        skipped.push(SkippedFile {
            path: String::new(),
            reason: format!("The slideshow folder could not be written to: {e}"),
        });
        return ImportOutcome { added, skipped };
    }

    for raw_path in paths {
        let path = Path::new(raw_path);
        let mut skip = |reason: String| skipped.push(SkippedFile {
            path: raw_path.clone(),
            reason,
        });

        if !path.is_file() {
            skip("That file couldn't be found.".to_string());
            continue;
        }
        let ext = extension_of(path);
        let category = match ext_category(&ext) {
            Some(c) => c,
            None => {
                skip(format!(
                    "“.{ext}” files aren't supported yet — photos (.jpg, .png, .heic), videos (.mp4, .mov), and songs (.mp3) work."
                ));
                continue;
            }
        };

        let id = Uuid::new_v4().simple().to_string();
        let stem = sanitize_stem(&path.file_stem().unwrap_or_default().to_string_lossy());

        let prepared: Prepared = if HEIC_EXTS.contains(&ext.as_str()) {
            let file_name = format!("{}/{id}-{}.jpg", MEDIA_DIR, stem);
            let dest = project_dir.join(&file_name);
            let src = path.to_path_buf();
            let job_dest = dest.clone();
            let converted = tokio::task::spawn_blocking(move || convert_heic_to_jpeg(&src, &job_dest))
                .await
                .map_err(|_| "The photo couldn't be opened.".to_string())
                .and_then(|r| r);
            match converted {
                Ok((w, h)) => Prepared {
                    file_name,
                    kind: AssetKind::Image,
                    width: Some(w),
                    height: Some(h),
                    duration: None,
                },
                Err(e) => {
                    let _ = std::fs::remove_file(&dest);
                    skip(e);
                    continue;
                }
            }
        } else {
            let info = match probe(bins, path).await {
                Ok(info) => info,
                Err(e) => {
                    skip(e);
                    continue;
                }
            };
            let kind = match decide_kind(&info) {
                Some(kind) => kind,
                None => {
                    skip("This file doesn't contain a picture or sound.".to_string());
                    continue;
                }
            };
            let matches_hint = match category {
                "image" => kind == AssetKind::Image,
                "video" => kind == AssetKind::Video,
                "audio" => kind == AssetKind::Audio,
                _ => false,
            };
            if !matches_hint {
                skip("This file isn't what its name suggests (it couldn't be used).".to_string());
                continue;
            }
            let file_name = format!("{}/{id}-{}.{}", MEDIA_DIR, stem, ext);
            if let Err(e) = std::fs::copy(path, project_dir.join(&file_name)) {
                skip(format!("The file couldn't be copied into the slideshow: {e}"));
                continue;
            }
            Prepared {
                file_name,
                kind,
                width: if kind == AssetKind::Audio { None } else { info.width },
                height: if kind == AssetKind::Audio { None } else { info.height },
                duration: match kind {
                    AssetKind::Video | AssetKind::Audio => info.duration.filter(|d| *d > 0.0),
                    AssetKind::Image => None,
                },
            }
        };

        let media_abs: PathBuf = project_dir.join(&prepared.file_name);
        if prepared.kind != AssetKind::Audio {
            let thumb = project_dir.join(THUMBS_DIR).join(format!("{id}.jpg"));
            if let Err(e) = make_thumbnail(bins, &media_abs, &thumb, prepared.kind, prepared.duration).await {
                let _ = std::fs::remove_file(&media_abs);
                skip(e);
                continue;
            }
        }

        let asset = Asset {
            id: id.clone(),
            file_name: prepared.file_name,
            kind: prepared.kind,
            width: prepared.width,
            height: prepared.height,
            duration: prepared.duration,
        };
        project.assets.insert(id.clone(), asset.clone());
        if prepared.kind != AssetKind::Audio {
            project.items.push(Item {
                id: Uuid::new_v4().simple().to_string(),
                asset_id: id.clone(),
                kind: prepared.kind,
                fit: Default::default(),
                duration: None,
                trim_start: 0.0,
            });
        } else if first_song.is_none() {
            first_song = Some(id.clone());
        }
        added.push(asset);
    }

    // Dropping a song with the photos sets it as the soundtrack (replaceable
    // later from the music card).
    if project.audio.is_none() {
        if let Some(song_id) = first_song {
            project.audio = Some(AudioSettings::new(song_id));
        }
    }

    ImportOutcome { added, skipped }
}

/// Adds the background song (copied into the project, becomes project.audio).
pub async fn import_song(
    bins: &Binaries,
    project_dir: &Path,
    project: &mut Project,
    raw_path: &str,
) -> Result<Asset, String> {
    let path = Path::new(raw_path);
    if !path.is_file() {
        return Err("That song file couldn't be found.".to_string());
    }
    let info = probe(bins, path).await?;
    if !info.has_audio {
        return Err("That file isn't a song (no sound found).".to_string());
    }
    let id = Uuid::new_v4().simple().to_string();
    let stem = sanitize_stem(&path.file_stem().unwrap_or_default().to_string_lossy());
    let ext = extension_of(path);
    let file_ext = if ext.is_empty() { "mp3".to_string() } else { ext };
    std::fs::create_dir_all(project_dir.join(MEDIA_DIR))
        .map_err(|e| format!("The slideshow folder could not be written to: {e}"))?;
    let file_name = format!("{}/{id}-{}.{}", MEDIA_DIR, stem, file_ext);
    std::fs::copy(path, project_dir.join(&file_name))
        .map_err(|e| format!("The song couldn't be copied into the slideshow: {e}"))?;
    let asset = Asset {
        id: id.clone(),
        file_name,
        kind: AssetKind::Audio,
        width: None,
        height: None,
        duration: info.duration.filter(|d| *d > 0.0),
    };
    project.assets.insert(id.clone(), asset.clone());
    project.audio = Some(AudioSettings::new(id));
    Ok(asset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_decided_from_probe_evidence() {
        let video = ProbeInfo {
            duration: Some(12.4),
            width: Some(1920),
            height: Some(1080),
            has_video: true,
            has_audio: true,
        };
        assert_eq!(decide_kind(&video), Some(AssetKind::Video));

        let photo = ProbeInfo {
            duration: None,
            width: Some(4032),
            height: Some(3024),
            has_video: true, // images probe as single video streams
            has_audio: false,
        };
        assert_eq!(decide_kind(&photo), Some(AssetKind::Image));

        let song = ProbeInfo {
            duration: Some(201.0),
            width: None,
            height: None,
            has_video: false,
            has_audio: true,
        };
        assert_eq!(decide_kind(&song), Some(AssetKind::Audio));

        let nothing = ProbeInfo {
            duration: None,
            width: None,
            height: None,
            has_video: false,
            has_audio: false,
        };
        assert_eq!(decide_kind(&nothing), None);
    }

    #[test]
    fn known_extensions_are_recognized() {
        assert_eq!(ext_category("jpg"), Some("image"));
        assert_eq!(ext_category("heic"), Some("image"));
        assert_eq!(ext_category("mov"), Some("video"));
        assert_eq!(ext_category("mp3"), Some("audio"));
        assert_eq!(ext_category("pdf"), None);
    }

    #[test]
    fn stems_are_folder_safe() {
        assert_eq!(sanitize_stem("beach 2019"), "beach 2019");
        assert_eq!(sanitize_stem("a/b*?"), "a-b--");
        assert_eq!(sanitize_stem("IMG_4862").len(), 8);
    }
}
