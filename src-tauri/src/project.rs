use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

pub const PROJECT_VERSION: u32 = 1;
pub const PROJECT_FILE: &str = "project.json";
pub const MEDIA_DIR: &str = "media";
pub const THUMBS_DIR: &str = "thumbs";
pub const CACHE_DIR: &str = "cache";
pub const PROJECT_EXTENSION: &str = ".lantern";

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Image,
    Video,
    Audio,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Fit {
    #[default]
    Contain,
    Cover,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "lowercase")]
pub enum Background {
    #[default]
    Black,
    White,
    Blur,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub crf: u32,
    pub preset: String,
    /// "auto" | "cpu" | "gpu" — resolved against the machine at export time.
    #[serde(default = "default_encoder")]
    pub encoder: String,
    pub background: Background,
    pub default_image_duration: f64,
    /// "none" | "crossfade" | "slide" | "checkerboard" | "kenburns" — one motion
    /// style for the whole show. Normalized by `normalize_transition`.
    #[serde(default = "default_transition")]
    pub transition: String,
}

fn default_encoder() -> String {
    "auto".to_string()
}

fn default_transition() -> String {
    "none".to_string()
}

/// The transition styles this app understands. Anything else clamps to "none".
pub const TRANSITIONS: &[&str] = &["none", "crossfade", "slide", "checkerboard", "kenburns"];

pub fn normalize_transition(value: &str) -> &'static str {
    TRANSITIONS
        .iter()
        .copied()
        .find(|t| *t == value)
        .unwrap_or("none")
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            width: 1920,
            height: 1080,
            fps: 30,
            crf: 21,
            preset: "medium".to_string(),
            encoder: "auto".to_string(),
            background: Background::Black,
            default_image_duration: 3.0,
            transition: "none".to_string(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    pub asset_id: String,
    pub volume: f64,
    pub fade_in: f64,
    pub fade_out: f64,
    #[serde(rename = "loop", default = "default_true")]
    pub loop_track: bool,
}

fn default_true() -> bool {
    true
}

impl AudioSettings {
    pub fn new(asset_id: String) -> Self {
        AudioSettings {
            asset_id,
            volume: 0.8,
            fade_in: 1.0,
            fade_out: 3.0,
            loop_track: true,
        }
    }
}

/// A media file that belongs to the project (copied into `media/` on import).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub id: String,
    /// Path relative to the project folder, e.g. `media/1f3a-beach.jpg`.
    pub file_name: String,
    pub kind: AssetKind,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// Seconds, for video and audio assets.
    pub duration: Option<f64>,
}

impl Asset {
    pub fn thumb_path(&self) -> Option<String> {
        match self.kind {
            AssetKind::Image | AssetKind::Video => Some(format!("{}/{id}.jpg", THUMBS_DIR, id = self.id)),
            AssetKind::Audio => None,
        }
    }
}

/// One position on the timeline. Images hold for `duration` (or the project
/// default); videos play `duration` seconds from `trimStart` (whole clip when
/// duration is null).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub asset_id: String,
    pub kind: AssetKind,
    pub fit: Fit,
    pub duration: Option<f64>,
    #[serde(default)]
    pub trim_start: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub version: u32,
    pub settings: Settings,
    pub audio: Option<AudioSettings>,
    pub items: Vec<Item>,
    pub assets: BTreeMap<String, Asset>,
}

impl Default for Project {
    fn default() -> Self {
        Project {
            version: PROJECT_VERSION,
            settings: Settings::default(),
            audio: None,
            items: Vec::new(),
            assets: BTreeMap::new(),
        }
    }
}

impl Project {
    /// Structural validation. Media file existence is checked by
    /// `ProjectCtx::validate_media` after load (it needs the folder).
    pub fn validate(&self) -> Result<(), String> {
        if self.version != PROJECT_VERSION {
            return Err(format!(
                "This slideshow was saved by a newer version of the app (format {}).",
                self.version
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for item in &self.items {
            if !ids.insert(item.id.as_str()) {
                return Err("The slideshow contains a duplicated entry.".to_string());
            }
            if !self.assets.contains_key(&item.asset_id) {
                return Err(
                    "The slideshow references a photo or video that is missing from the folder."
                        .to_string(),
                );
            }
        }
        if let Some(audio) = &self.audio {
            match self.assets.get(&audio.asset_id) {
                Some(asset) if asset.kind == AssetKind::Audio => {}
                _ => return Err("The chosen song is missing from the folder.".to_string()),
            }
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct ProjectCtx {
    pub dir: PathBuf,
    pub project: Project,
}

impl ProjectCtx {
    pub fn media_path(&self, asset: &Asset) -> PathBuf {
        self.dir.join(&asset.file_name)
    }

    pub fn thumb_path(&self, asset: &Asset) -> Option<PathBuf> {
        asset.thumb_path().map(|p| self.dir.join(p))
    }

    pub fn validate_media(&self) -> Result<(), String> {
        for asset in self.project.assets.values() {
            if !self.media_path(asset).exists() {
                return Err(format!(
                    "A file is missing from this slideshow: {}",
                    asset.file_name
                ));
            }
        }
        self.project.validate()
    }

    pub fn save(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.project)
            .map_err(|e| format!("Could not save the slideshow: {e}"))?;
        std::fs::write(self.dir.join(PROJECT_FILE), json)
            .map_err(|e| format!("Could not save the slideshow: {e}"))?;
        Ok(())
    }

    pub fn load(dir: &Path) -> Result<ProjectCtx, String> {
        let file = dir.join(PROJECT_FILE);
        let text = std::fs::read_to_string(&file).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => format!("No slideshow found in {:?}", dir.display()),
            other => format!("Could not open the slideshow: {other}"),
        })?;
        let project: Project = serde_json::from_str(&text)
            .map_err(|e| format!("The slideshow file is damaged ({e})."))?;
        let ctx = ProjectCtx {
            dir: dir.to_path_buf(),
            project,
        };
        ctx.validate_media()?;
        let ctx = ctx.gc_orphans();
        Ok(ctx)
    }

    /// Drops asset records (and their files) that nothing references anymore.
    /// Removing a photo keeps its files during the session so Undo can bring it
    /// back; leftovers are reclaimed when the project is next opened.
    fn gc_orphans(mut self) -> ProjectCtx {
        let referenced: std::collections::HashSet<&String> = self
            .project
            .items
            .iter()
            .map(|item| &item.asset_id)
            .chain(self.project.audio.iter().map(|audio| &audio.asset_id))
            .collect();
        let orphaned: Vec<String> = self
            .project
            .assets
            .iter()
            .filter(|(id, _)| !referenced.contains(id))
            .map(|(id, _)| id.clone())
            .collect();
        for id in &orphaned {
            if let Some(asset) = self.project.assets.remove(id) {
                let _ = std::fs::remove_file(self.dir.join(&asset.file_name));
                let _ = std::fs::remove_file(
                    self.dir.join(THUMBS_DIR).join(format!("{id}.jpg")),
                );
            }
        }
        if !orphaned.is_empty() {
            let _ = self.save();
        }
        self
    }

    /// Creates a new project folder `<parent>/<name>.lantern`.
    pub fn create(parent: &Path, name: &str) -> Result<ProjectCtx, String> {
        let clean = sanitize_project_name(name)?;
        let dir = parent.join(format!("{clean}{PROJECT_EXTENSION}"));
        if dir.exists() {
            return Err(format!(
                "“{clean}” already exists. Pick a different name or open it from the list."
            ));
        }
        std::fs::create_dir(&dir).map_err(|e| format!("Could not create the folder: {e}"))?;
        for sub in [MEDIA_DIR, THUMBS_DIR, CACHE_DIR] {
            if let Err(e) = std::fs::create_dir(dir.join(sub)) {
                let _ = std::fs::remove_dir(&dir);
                return Err(format!("Could not create the folder: {e}"));
            }
        }
        let ctx = ProjectCtx {
            dir,
            project: Project::default(),
        };
        ctx.save()?;
        Ok(ctx)
    }
}

pub fn sanitize_project_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("Please give the slideshow a name.".to_string());
    }
    let cleaned: String = trimmed
        .chars()
        .map(|c| {
            if c == '/' || c == '\\' || c == ':' || c == '\0' {
                '-'
            } else {
                c
            }
        })
        .collect();
    if cleaned.chars().all(|c| c == '.' || c == ' ' || c == '-') {
        return Err("The name needs at least one letter or number.".to_string());
    }
    Ok(cleaned)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct RecentProject {
    pub name: String,
    pub path: String,
    pub updated_at: u64,
}

pub fn recent_projects(app_data: &Path) -> Vec<RecentProject> {
    let Ok(text) = std::fs::read_to_string(app_data.join("recents.json")) else {
        return Vec::new();
    };
    let Ok(list) = serde_json::from_str::<Vec<RecentProject>>(&text) else {
        return Vec::new();
    };
    let mut live: Vec<RecentProject> = list
        .into_iter()
        .filter(|r| Path::new(&r.path).is_dir())
        .collect();
    live.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
    live.truncate(12);
    live
}

pub fn touch_recent(app_data: &Path, ctx: &ProjectCtx) {
    let mut list = recent_projects(app_data);
    let path = ctx.dir.display().to_string();
    list.retain(|r| r.path != path);
    let name = ctx
        .dir
        .file_name()
        .map(|n| n.to_string_lossy().trim_end_matches(PROJECT_EXTENSION).to_string())
        .unwrap_or_else(|| "Slideshow".to_string());
    list.push(RecentProject {
        name,
        path,
        updated_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0),
    });
    list.truncate(12);
    if let Ok(json) = serde_json::to_string_pretty(&list) {
        let _ = std::fs::write(app_data.join("recents.json"), json);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_project_roundtrips_through_json() {
        let project = Project::default();
        let json = serde_json::to_string(&project).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back, project);
        assert!(json.contains("\"defaultImageDuration\":3.0"));
    }

    #[test]
    fn item_without_explicit_duration_defaults_to_null() {
        let json = r#"{"version":1,"settings":{"width":1920,"height":1080,"fps":30,"crf":21,"preset":"medium","background":"black","defaultImageDuration":3.0},"audio":null,"items":[{"id":"i1","assetId":"a1","kind":"image","fit":"contain","duration":null,"trimStart":0.0}],"assets":{}}"#;
        let project: Project = serde_json::from_str(json).unwrap();
        assert_eq!(project.items.len(), 1);
        assert_eq!(project.items[0].duration, None);
        assert_eq!(project.items[0].fit, Fit::Contain);
    }

    #[test]
    fn missing_asset_reference_is_rejected() {
        let mut project = Project::default();
        project.items.push(Item {
            id: "i1".into(),
            asset_id: "gone".into(),
            kind: AssetKind::Image,
            fit: Fit::Contain,
            duration: None,
            trim_start: 0.0,
        });
        assert!(project.validate().is_err());
    }

    #[test]
    fn duplicated_item_is_rejected() {
        let asset = Asset {
            id: "a1".into(),
            file_name: "media/a1-x.jpg".into(),
            kind: AssetKind::Image,
            width: None,
            height: None,
            duration: None,
        };
        let mut project = Project::default();
        project.assets.insert("a1".into(), asset.clone());
        let item = Item {
            id: "i1".into(),
            asset_id: "a1".into(),
            kind: AssetKind::Image,
            fit: Fit::Contain,
            duration: None,
            trim_start: 0.0,
        };
        project.items.push(item.clone());
        project.items.push(item);
        assert!(project.validate().is_err());
    }

    #[test]
    fn names_are_safe_for_folders() {
        assert_eq!(sanitize_project_name("Dad's Life").unwrap(), "Dad's Life");
        assert_eq!(sanitize_project_name("a/b:c").unwrap(), "a-b-c");
        assert!(sanitize_project_name("   ").is_err());
        assert!(sanitize_project_name("...").is_err());
    }

    #[test]
    fn transition_defaults_to_none_when_absent() {
        let json = r#"{"version":1,"settings":{"width":1920,"height":1080,"fps":30,"crf":21,"preset":"medium","background":"black","defaultImageDuration":3.0},"audio":null,"items":[],"assets":{}}"#;
        let project: Project = serde_json::from_str(json).unwrap();
        assert_eq!(project.settings.transition, "none");
    }

    #[test]
    fn transition_roundtrips_and_normalizes() {
        let mut project = Project::default();
        project.settings.transition = "checkerboard".to_string();
        let json = serde_json::to_string(&project).unwrap();
        let back: Project = serde_json::from_str(&json).unwrap();
        assert_eq!(back.settings.transition, "checkerboard");
        assert_eq!(normalize_transition("kenburns"), "kenburns");
        assert_eq!(normalize_transition("wiggle"), "none");
        assert_eq!(normalize_transition(""), "none");
    }
}
