pub mod cli;
mod engine;
pub mod export;
pub mod ffmpeg;
pub mod media;
pub mod project;
pub mod state;
pub mod timeline;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

use crate::ffmpeg::Binaries;
use crate::project::{Background, Fit, Project, ProjectCtx, RecentProject};
use crate::state::AppState;
use crate::timeline::Plan;

/// Everything the frontend needs after any mutation: the project plus a fresh
/// render plan (the backend is the single source of timeline truth).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ProjectSnapshot {
    dir: String,
    project: Project,
    plan: Plan,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ImportResult {
    snapshot: ProjectSnapshot,
    added: usize,
    skipped: Vec<media::SkippedFile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SettingsPatch {
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    crf: Option<u32>,
    #[serde(default)]
    preset: Option<String>,
    #[serde(default)]
    background: Option<Background>,
    #[serde(default)]
    default_image_duration: Option<f64>,
}

const PRESETS: &[&str] = &[
    "ultrafast", "superfast", "veryfast", "faster", "fast", "medium", "slow", "slower",
    "veryslow",
];

fn app_data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    let dir = app
        .path()
        .app_data_dir()
        .map_err(|e| format!("Could not locate the app data folder: {e}"))?;
    let _ = std::fs::create_dir_all(&dir);
    Ok(dir)
}

fn snapshot_of(ctx: &ProjectCtx) -> Result<ProjectSnapshot, String> {
    let plan = timeline::plan(ctx)?;
    Ok(ProjectSnapshot {
        dir: ctx.dir.display().to_string(),
        project: ctx.project.clone(),
        plan,
    })
}

fn current_ctx(app: &AppHandle) -> Result<ProjectCtx, String> {
    let state = app.state::<AppState>();
    let guard = state
        .project
        .lock()
        .map_err(|_| "The slideshow is busy — try again.".to_string())?;
    guard.clone().ok_or("No slideshow is open.".to_string())
}

fn remember(app: &AppHandle, ctx: &ProjectCtx) {
    if let Ok(dir) = app_data_dir(app) {
        project::touch_recent(&dir, ctx);
    }
}

fn store_project(app: &AppHandle, ctx: ProjectCtx) -> Result<(), String> {
    let state = app.state::<AppState>();
    *state
        .project
        .lock()
        .map_err(|_| "The slideshow is busy — try again.".to_string())? = Some(ctx);
    Ok(())
}

fn install_open_project(app: &AppHandle, ctx: ProjectCtx) -> Result<ProjectSnapshot, String> {
    // the asset: protocol needs read access to this project's folder
    let _ = app
        .asset_protocol_scope()
        .allow_directory(&ctx.dir, true)
        .map_err(|e| eprintln!("[asset-scope] {e}"));
    let snapshot = snapshot_of(&ctx)?;
    remember(app, &ctx);
    store_project(app, ctx)?;
    Ok(snapshot)
}

/// Apply a mutation to a cloned project, persist it, and hand back a snapshot.
fn with_project(
    app: &AppHandle,
    f: impl FnOnce(&ProjectCtx) -> Result<Project, String>,
) -> Result<ProjectSnapshot, String> {
    let ctx = current_ctx(app)?;
    let updated = f(&ctx)?;
    let next = ProjectCtx {
        dir: ctx.dir.clone(),
        project: updated,
    };
    next.save()?;
    let snapshot = snapshot_of(&next)?;
    remember(app, &next);
    store_project(app, next)?;
    Ok(snapshot)
}

// ---------------------------------------------------------------- launch

#[tauri::command]
fn recent_projects(app: AppHandle) -> Vec<RecentProject> {
    match app_data_dir(&app) {
        Ok(dir) => project::recent_projects(&dir),
        Err(_) => Vec::new(),
    }
}

/// "Continue where you left off": reopen the most recent project that is intact.
#[tauri::command]
fn startup_project(app: AppHandle) -> Option<ProjectSnapshot> {
    for recent in recent_projects(app.clone()) {
        match ProjectCtx::load(Path::new(&recent.path)) {
            Ok(ctx) => match install_open_project(&app, ctx) {
                Ok(snapshot) => return Some(snapshot),
                Err(e) => eprintln!("[startup] {}: {e}", recent.path),
            },
            Err(e) => eprintln!("[startup] {}: {e}", recent.path),
        }
    }
    None
}

#[tauri::command]
async fn project_new(app: AppHandle, parent_dir: String, name: String) -> Result<ProjectSnapshot, String> {
    let ctx = ProjectCtx::create(Path::new(&parent_dir), &name)?;
    install_open_project(&app, ctx)
}

#[tauri::command]
async fn project_open(app: AppHandle, dir: String) -> Result<ProjectSnapshot, String> {
    let ctx = ProjectCtx::load(Path::new(&dir))?;
    install_open_project(&app, ctx)
}

#[tauri::command]
fn project_close(app: AppHandle) {
    let state = app.state::<AppState>();
    if let Ok(mut guard) = state.project.lock() {
        *guard = None;
    }
}

/// Undo support: put a previously captured project state back in place
/// (validated, saved, and reflected in a fresh snapshot).
#[tauri::command]
fn project_restore(app: AppHandle, project: Project) -> Result<ProjectSnapshot, String> {
    let ctx = current_ctx(&app)?;
    let next = ProjectCtx {
        dir: ctx.dir.clone(),
        project,
    };
    next.validate_media()?;
    next.save()?;
    let snapshot = snapshot_of(&next)?;
    remember(&app, &next);
    store_project(&app, next)?;
    Ok(snapshot)
}

// ---------------------------------------------------------------- import

#[tauri::command]
async fn import_media(app: AppHandle, paths: Vec<String>) -> Result<ImportResult, String> {
    let mut ctx = current_ctx(&app)?;
    let bins = Binaries::resolve(app.path().resource_dir().ok().as_deref());
    let outcome = media::import(&bins, &ctx.dir, &mut ctx.project, &paths).await;
    ctx.save()?;
    remember(&app, &ctx);
    let snapshot = snapshot_of(&ctx)?;
    store_project(&app, ctx)?;
    Ok(ImportResult {
        snapshot,
        added: outcome.added.len(),
        skipped: outcome.skipped,
    })
}

#[tauri::command]
async fn audio_set(app: AppHandle, path: String) -> Result<ProjectSnapshot, String> {
    let mut ctx = current_ctx(&app)?;
    let bins = Binaries::resolve(app.path().resource_dir().ok().as_deref());
    media::import_song(&bins, &ctx.dir, &mut ctx.project, &path).await?;
    ctx.save()?;
    remember(&app, &ctx);
    let snapshot = snapshot_of(&ctx)?;
    store_project(&app, ctx)?;
    Ok(snapshot)
}

#[tauri::command]
fn audio_remove(app: AppHandle) -> Result<ProjectSnapshot, String> {
    with_project(&app, |ctx| {
        let mut project = ctx.project.clone();
        project.audio = None;
        Ok(project)
    })
}

#[tauri::command]
fn audio_set_volume(app: AppHandle, volume: f64) -> Result<ProjectSnapshot, String> {
    with_project(&app, |ctx| {
        let mut project = ctx.project.clone();
        let audio = project.audio.as_mut().ok_or("No song is set.")?;
        audio.volume = volume.clamp(0.0, 1.5);
        Ok(project)
    })
}

// ---------------------------------------------------------------- timeline

#[tauri::command]
fn item_move(app: AppHandle, from: usize, to: usize) -> Result<ProjectSnapshot, String> {
    with_project(&app, |ctx| {
        let mut project = ctx.project.clone();
        if from >= project.items.len() || to >= project.items.len() {
            return Err("That move is out of range.".to_string());
        }
        let item = project.items.remove(from);
        project.items.insert(to, item);
        Ok(project)
    })
}

#[tauri::command]
fn item_remove(app: AppHandle, item_id: String) -> Result<ProjectSnapshot, String> {
    with_project(&app, |ctx| {
        let mut project = ctx.project.clone();
        let before = project.items.len();
        project.items.retain(|item| item.id != item_id);
        if project.items.len() == before {
            return Err("That photo is no longer in the slideshow.".to_string());
        }
        // Files stay put: undo may bring the item back, and `load` reclaims
        // anything still orphaned the next time the project opens.
        Ok(project)
    })
}

#[tauri::command]
fn item_set_duration(app: AppHandle, item_id: String, duration: f64) -> Result<ProjectSnapshot, String> {
    with_project(&app, |ctx| {
        let mut project = ctx.project.clone();
        let item = project
            .items
            .iter_mut()
            .find(|item| item.id == item_id)
            .ok_or("That photo is no longer in the slideshow.")?;
        if !matches!(item.kind, project::AssetKind::Image) {
            return Err("Video length comes from the clip itself.".to_string());
        }
        item.duration = Some(duration.clamp(0.5, 120.0));
        Ok(project)
    })
}

#[tauri::command]
fn item_set_trim(
    app: AppHandle,
    item_id: String,
    trim_start: Option<f64>,
    duration: Option<f64>,
) -> Result<ProjectSnapshot, String> {
    with_project(&app, |ctx| {
        let mut project = ctx.project.clone();
        let item = project
            .items
            .iter_mut()
            .find(|item| item.id == item_id)
            .ok_or("That clip is no longer in the slideshow.")?;
        if !matches!(item.kind, project::AssetKind::Video) {
            return Err("Only videos can be trimmed.".to_string());
        }
        let full = project
            .assets
            .get(&item.asset_id)
            .and_then(|asset| asset.duration)
            .ok_or("This clip has no measurable length.")?;
        let start = trim_start.unwrap_or(item.trim_start).clamp(0.0, (full - 0.1).max(0.0));
        item.trim_start = start;
        item.duration = match duration {
            Some(d) => Some(d.clamp(0.1, full - start)),
            None => None,
        };
        Ok(project)
    })
}

#[tauri::command]
fn item_set_fit(app: AppHandle, item_id: String, fit: Fit) -> Result<ProjectSnapshot, String> {
    with_project(&app, |ctx| {
        let mut project = ctx.project.clone();
        let item = project
            .items
            .iter_mut()
            .find(|item| item.id == item_id)
            .ok_or("That photo is no longer in the slideshow.")?;
        item.fit = fit;
        Ok(project)
    })
}

#[tauri::command]
fn make_all_photos(app: AppHandle, duration: f64) -> Result<ProjectSnapshot, String> {
    with_project(&app, |ctx| {
        let mut project = ctx.project.clone();
        let duration = duration.clamp(0.5, 120.0);
        for item in project.items.iter_mut() {
            if matches!(item.kind, project::AssetKind::Image) {
                item.duration = Some(duration);
            }
        }
        project.settings.default_image_duration = duration;
        Ok(project)
    })
}

#[tauri::command]
fn settings_set(app: AppHandle, patch: SettingsPatch) -> Result<ProjectSnapshot, String> {
    with_project(&app, |ctx| {
        let mut project = ctx.project.clone();
        if let Some(width) = patch.width {
            if !(480..=4096).contains(&width) || width % 2 != 0 {
                return Err("Video width must be an even number between 480 and 4096.".to_string());
            }
            project.settings.width = width;
        }
        if let Some(height) = patch.height {
            if !(480..=4096).contains(&height) || height % 2 != 0 {
                return Err("Video height must be an even number between 480 and 4096.".to_string());
            }
            project.settings.height = height;
        }
        if let Some(crf) = patch.crf {
            if !(14..=35).contains(&crf) {
                return Err("Quality must be between 14 and 35.".to_string());
            }
            project.settings.crf = crf;
        }
        if let Some(preset) = patch.preset {
            if !PRESETS.contains(&preset.as_str()) {
                return Err("Unknown speed preset.".to_string());
            }
            project.settings.preset = preset;
        }
        if let Some(background) = patch.background {
            project.settings.background = background;
        }
        if let Some(duration) = patch.default_image_duration {
            project.settings.default_image_duration = duration.clamp(0.5, 120.0);
        }
        Ok(project)
    })
}

// ---------------------------------------------------------------- export

#[tauri::command]
async fn export_start(app: AppHandle, output_path: String) -> Result<(), String> {
    let ctx = current_ctx(&app)?;
    let ctl = {
        let state = app.state::<AppState>();
        let ctl = Arc::clone(&state.export);
        ctl.begin()?;
        ctl
    };

    let mut out = PathBuf::from(&output_path);
    let has_mp4_ext = out
        .extension()
        .map(|e| e.to_string_lossy().eq_ignore_ascii_case("mp4"))
        .unwrap_or(false);
    if !has_mp4_ext {
        out.set_extension("mp4");
    }

    let resource_dir = app.path().resource_dir().ok();
    let emitter = app.clone();
    tauri::async_runtime::spawn(async move {
        let bins = Binaries::resolve(resource_dir.as_deref());
        let mut sink = |event: &export::ProgressEvent| {
            let _ = emitter.emit("export-progress", event);
        };
        let result = export::export_project(&bins, &ctx, &out, &ctl, &mut sink).await;
        ctl.end();
        match result {
            Ok(summary) => {
                let _ = emitter.emit("export-done", &summary);
            }
            Err(e) if e == export::CANCELLED => {
                let _ = emitter.emit("export-cancelled", ());
            }
            Err(e) => {
                let _ = emitter.emit("export-error", serde_json::json!({ "message": e }));
            }
        }
    });
    Ok(())
}

#[tauri::command]
fn export_cancel(app: AppHandle) -> Result<(), String> {
    app.state::<AppState>().export.request_cancel();
    Ok(())
}

// ---------------------------------------------------------------- setup

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .manage(AppState::default())
        .setup(|app| {
            // Log which engine we found; a missing one surfaces later with a
            // clear message rather than blocking the UI from opening.
            match tauri::async_runtime::block_on(engine::engine_status(app.handle().clone())) {
                Ok(status) => eprintln!("[setup] engine ready: {}", status.version),
                Err(e) => eprintln!("[setup] engine problem: {e}"),
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            engine::engine_status,
            recent_projects,
            startup_project,
            project_new,
            project_open,
            project_close,
            project_restore,
            import_media,
            audio_set,
            audio_remove,
            audio_set_volume,
            item_move,
            item_remove,
            item_set_duration,
            item_set_trim,
            item_set_fit,
            make_all_photos,
            settings_set,
            export_start,
            export_cancel,
        ])
        .run(tauri::generate_context!())
        .expect("error while running the slideshow app");
}
