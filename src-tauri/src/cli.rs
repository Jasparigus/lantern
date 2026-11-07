//! Headless command surface for smoke tests, CI, and shell workflows:
//!   lantern engine
//!   lantern new <parent-dir> <name>
//!   lantern import <project-dir> <file>...
//!   lantern export <project-dir> <output.mp4>

use std::path::{Path, PathBuf};
use std::sync::Arc;

use tauri::async_runtime;

use crate::export;
use crate::ffmpeg::Binaries;
use crate::media;
use crate::project::ProjectCtx;
use crate::state::ExportCtl;

pub fn run(args: &[String]) -> i32 {
    match args.first().map(|s| s.as_str()) {
        Some("engine") => engine(),
        Some("new") if args.len() == 3 => new_project(&args[1], &args[2]),
        Some("import") if args.len() >= 3 => import(&args[1], &args[2..]),
        Some("export") if args.len() == 3 => export_project(&args[1], &args[2]),
        _ => {
            usage();
            2
        }
    }
}

fn usage() {
    eprintln!(
        "usage:\n  \x73lideshowtool engine\n  \x73lideshowtool new <parent-dir> <name>\n  \x73lideshowtool import <project-dir> <file>...\n  \x73lideshowtool export <project-dir> <output.mp4>"
    );
}

fn engine() -> i32 {
    let bins = Binaries::resolve(None);
    match async_runtime::block_on(crate::ffmpeg::run_version(&bins.ffmpeg)) {
        Ok(version) => {
            println!("ffmpeg {version} — {}", bins.ffmpeg.display());
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn new_project(parent: &str, name: &str) -> i32 {
    match ProjectCtx::create(Path::new(parent), name) {
        Ok(ctx) => {
            println!("Created {}", ctx.dir.display());
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

fn import(project_dir: &str, files: &[String]) -> i32 {
    let mut ctx = match ProjectCtx::load(Path::new(project_dir)) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let bins = Binaries::resolve(None);
    let outcome = async_runtime::block_on(media::import(&bins, &ctx.dir, &mut ctx.project, files));
    for asset in &outcome.added {
        println!("Added {}", asset.file_name);
    }
    for skipped in &outcome.skipped {
        eprintln!("Skipped {}: {}", skipped.path, skipped.reason);
    }
    if outcome.added.is_empty() {
        return 1;
    }
    if let Err(e) = ctx.save() {
        eprintln!("{e}");
        return 1;
    }
    println!(
        "Saved {} ({} item(s))",
        ctx.dir.display(),
        ctx.project.items.len()
    );
    if !outcome.skipped.is_empty() {
        1
    } else {
        0
    }
}

fn export_project(project_dir: &str, output: &str) -> i32 {
    let ctx = match ProjectCtx::load(Path::new(project_dir)) {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    let bins = Binaries::resolve(None);
    let ctl = Arc::new(ExportCtl::new());
    match ctl.begin() {
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
        Ok(()) => {}
    }
    let out_path = PathBuf::from(output);
    let mut progress = |event: &export::ProgressEvent| println!("{}", event.label);
    let result =
        async_runtime::block_on(export::export_project(&bins, &ctx, &out_path, &ctl, &mut progress));
    ctl.end();
    match result {
        Ok(summary) => {
            println!(
                "Saved {} ({:.1} seconds)",
                summary.out_path, summary.total_duration
            );
            0
        }
        Err(e) => {
            eprintln!("{e}");
            if e == export::CANCELLED {
                3
            } else {
                1
            }
        }
    }
}
