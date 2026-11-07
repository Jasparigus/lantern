use serde::Serialize;

use crate::project::{AssetKind, ProjectCtx};

pub const MIN_SEGMENT: f64 = 0.1;
pub const MAX_IMAGE: f64 = 600.0;

/// A timeline position resolved to concrete files and seconds — what the exporter
/// and the preview player both consume.
#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlannedItem {
    pub item_id: String,
    pub asset_id: String,
    pub kind: AssetKind,
    pub media_path: String,
    pub thumbnail_path: Option<String>,
    pub trim_start: f64,
    pub duration: f64,
}

#[derive(Serialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub items: Vec<PlannedItem>,
    pub total_duration: f64,
}

pub fn plan(ctx: &ProjectCtx) -> Result<Plan, String> {
    let project = &ctx.project;
    let mut items = Vec::with_capacity(project.items.len());
    let mut total = 0.0f64;

    for item in &project.items {
        let asset = project
            .assets
            .get(&item.asset_id)
            .ok_or("The slideshow references a photo or video that is missing.")?;
        let media = ctx.media_path(asset);
        if !media.exists() {
            return Err(format!(
                "A file is missing from this slideshow: {}",
                asset.file_name
            ));
        }

        let duration = match asset.kind {
            AssetKind::Image => item
                .duration
                .unwrap_or(project.settings.default_image_duration)
                .clamp(MIN_SEGMENT, MAX_IMAGE),
            AssetKind::Video => {
                let full = asset.duration.unwrap_or(0.0);
                if full <= 0.0 {
                    return Err(format!(
                        "“{}” has no measurable length and can't be added to the slideshow.",
                        asset.file_name
                    ));
                }
                if item.trim_start >= full {
                    return Err(format!(
                        "The start point for “{}” is past its end.",
                        asset.file_name
                    ));
                }
                item.duration
                    .unwrap_or(full - item.trim_start)
                    .max(MIN_SEGMENT)
                    .min(full - item.trim_start)
            }
            AssetKind::Audio => {
                return Err("A song can't be placed on the slideshow strip.".to_string());
            }
        };

        total += duration;
        items.push(PlannedItem {
            item_id: item.id.clone(),
            asset_id: asset.id.clone(),
            kind: asset.kind,
            media_path: media.display().to_string(),
            thumbnail_path: ctx
                .thumb_path(asset)
                .map(|p| p.display().to_string()),
            trim_start: item.trim_start,
            duration,
        });
    }

    if let Some(audio) = &project.audio {
        match project.assets.get(&audio.asset_id) {
            Some(asset) if asset.kind == AssetKind::Audio => {}
            _ => return Err("The chosen song is missing from the folder.".to_string()),
        }
    }

    Ok(Plan {
        items,
        total_duration: total,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::{Asset, Item, Project};
    use std::collections::BTreeMap;

    fn ctx_with(assets: Vec<Asset>, items: Vec<Item>) -> ProjectCtx {
        let mut map = BTreeMap::new();
        for asset in assets {
            map.insert(asset.id.clone(), asset);
        }
        ProjectCtx {
            dir: std::env::current_dir().unwrap(),
            project: Project {
                items,
                assets: map,
                ..Default::default()
            },
        }
    }

    fn image_asset(id: &str) -> Asset {
        Asset {
            id: id.into(),
            // Relative paths inside the project dir; tests point them at real
            // files by resolving against the crate dir.
            file_name: "Cargo.toml".into(),
            kind: AssetKind::Image,
            width: Some(800),
            height: Some(600),
            duration: None,
        }
    }

    fn video_asset(id: &str, duration: f64) -> Asset {
        Asset {
            id: id.into(),
            file_name: "Cargo.toml".into(),
            kind: AssetKind::Video,
            width: Some(1280),
            height: Some(720),
            duration: Some(duration),
        }
    }

    fn item(id: &str, asset_id: &str, kind: AssetKind) -> Item {
        Item {
            id: id.into(),
            asset_id: asset_id.into(),
            kind,
            fit: Default::default(),
            duration: None,
            trim_start: 0.0,
        }
    }

    #[test]
    fn images_use_project_default_and_overrides() {
        let ctx = ctx_with(
            vec![image_asset("a1"), image_asset("a2")],
            vec![item("i1", "a1", AssetKind::Image), {
                let mut i = item("i2", "a2", AssetKind::Image);
                i.duration = Some(7.5);
                i
            }],
        );
        let plan = plan(&ctx).unwrap();
        assert_eq!(plan.items[0].duration, 3.0);
        assert_eq!(plan.items[1].duration, 7.5);
        assert!((plan.total_duration - 10.5).abs() < 1e-9);
    }

    #[test]
    fn video_without_duration_plays_from_trim_to_the_end() {
        let ctx = ctx_with(
            vec![video_asset("a1", 12.4)],
            vec![{
                let mut i = item("i1", "a1", AssetKind::Video);
                i.trim_start = 2.0;
                i
            }],
        );
        let plan = plan(&ctx).unwrap();
        assert!((plan.items[0].duration - 10.4).abs() < 1e-9);
    }

    #[test]
    fn video_clamps_explicit_duration_and_rejects_bad_trims() {
        let ctx = ctx_with(
            vec![video_asset("a1", 10.0)],
            vec![{
                let mut i = item("i1", "a1", AssetKind::Video);
                i.trim_start = 3.0;
                i.duration = Some(99.0);
                i
            }],
        );
        let schedule = plan(&ctx).unwrap();
        assert!((schedule.items[0].duration - 7.0).abs() < 1e-9);

        let bad = ctx_with(
            vec![video_asset("a1", 5.0)],
            vec![{
                let mut i = item("i1", "a1", AssetKind::Video);
                i.trim_start = 5.0;
                i
            }],
        );
        assert!(plan(&bad).is_err());
    }

    #[test]
    fn missing_asset_file_is_reported_by_name() {
        let mut asset = image_asset("a1");
        asset.file_name = "media/does-not-exist.jpg".into();
        let ctx = ctx_with(vec![asset], vec![item("i1", "a1", AssetKind::Image)]);
        let err = plan(&ctx).unwrap_err();
        assert!(err.contains("does-not-exist.jpg"));
    }

    #[test]
    fn song_cannot_sit_on_the_strip() {
        let mut asset = image_asset("a1");
        asset.file_name = "Cargo.toml".into();
        asset.kind = AssetKind::Audio;
        let ctx = ctx_with(vec![asset], vec![item("i1", "a1", AssetKind::Audio)]);
        assert!(plan(&ctx).is_err());
    }
}
