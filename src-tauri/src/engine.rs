use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::ffmpeg::{self, Binaries};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EngineStatus {
    pub source: String,
    pub path: String,
    pub version: String,
}

/// Reports which ffmpeg the app will actually use (bundled sidecar, explicit
/// override, or system PATH) so engine problems are diagnosable from the UI.
#[tauri::command]
pub async fn engine_status(app: AppHandle) -> Result<EngineStatus, String> {
    let bins = Binaries::resolve(app.path().resource_dir().ok().as_deref());
    let version = ffmpeg::run_version(&bins.ffmpeg).await?;
    let source = if bins
        .ffmpeg
        .file_name()
        .map(|n| n.to_string_lossy().starts_with("ffmpeg-"))
        .unwrap_or(false)
    {
        "bundled"
    } else {
        "system"
    };
    eprintln!(
        "[engine] ffmpeg {version} via {source}: {}",
        bins.ffmpeg.display()
    );
    Ok(EngineStatus {
        source: source.to_string(),
        path: bins.ffmpeg.display().to_string(),
        version,
    })
}
