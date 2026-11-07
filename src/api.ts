import { invoke } from "@tauri-apps/api/core";
import type {
  EngineStatus,
  Fit,
  ImportResult,
  Project,
  RecentProject,
  SettingsPatch,
  Snapshot,
} from "./types";

/// Typed wrappers over the Rust commands. Every mutation returns a fresh
/// snapshot so the UI can never drift from backend truth.
export const api = {
  engineStatus: () => invoke<EngineStatus>("engine_status"),

  startupProject: () => invoke<Snapshot | null>("startup_project"),
  recentProjects: () => invoke<RecentProject[]>("recent_projects"),
  projectNew: (parentDir: string, name: string) =>
    invoke<Snapshot>("project_new", { parentDir, name }),
  projectOpen: (dir: string) => invoke<Snapshot>("project_open", { dir }),
  projectClose: () => invoke<void>("project_close"),
  projectRestore: (project: Project) =>
    invoke<Snapshot>("project_restore", { project }),

  importMedia: (paths: string[]) =>
    invoke<ImportResult>("import_media", { paths }),

  audioSet: (path: string) => invoke<Snapshot>("audio_set", { path }),
  audioRemove: () => invoke<Snapshot>("audio_remove"),
  audioSetVolume: (volume: number) =>
    invoke<Snapshot>("audio_set_volume", { volume }),

  itemMove: (from: number, to: number) =>
    invoke<Snapshot>("item_move", { from, to }),
  itemRemove: (itemId: string) => invoke<Snapshot>("item_remove", { itemId }),
  itemSetDuration: (itemId: string, duration: number) =>
    invoke<Snapshot>("item_set_duration", { itemId, duration }),
  itemSetTrim: (
    itemId: string,
    trimStart: number | null,
    duration: number | null,
  ) => invoke<Snapshot>("item_set_trim", { itemId, trimStart, duration }),
  itemSetFit: (itemId: string, fit: Fit) =>
    invoke<Snapshot>("item_set_fit", { itemId, fit }),
  makeAllPhotos: (duration: number) =>
    invoke<Snapshot>("make_all_photos", { duration }),

  settingsSet: (patch: SettingsPatch) =>
    invoke<Snapshot>("settings_set", { patch }),

  exportStart: (outputPath: string) =>
    invoke<void>("export_start", { outputPath }),
  exportCancel: () => invoke<void>("export_cancel"),
};
