// Mirror of the Rust serde shapes (camelCase on the wire).

export type AssetKind = "image" | "video" | "audio";
export type Fit = "contain" | "cover";
export type Background = "black" | "white" | "blur";
export type Transition = "none" | "crossfade" | "slide" | "checkerboard" | "kenburns";

export interface Asset {
  id: string;
  fileName: string;
  kind: AssetKind;
  width: number | null;
  height: number | null;
  duration: number | null;
}

export interface Item {
  id: string;
  assetId: string;
  kind: AssetKind;
  fit: Fit;
  duration: number | null;
  trimStart: number;
}

export interface Settings {
  width: number;
  height: number;
  fps: number;
  crf: number;
  preset: string;
  encoder: "auto" | "cpu" | "gpu";
  background: Background;
  defaultImageDuration: number;
  transition: Transition;
}

export interface AudioSettings {
  assetId: string;
  volume: number;
  fadeIn: number;
  fadeOut: number;
  loop: boolean;
}

export interface Project {
  version: number;
  settings: Settings;
  audio: AudioSettings | null;
  items: Item[];
  assets: Record<string, Asset>;
}

export interface PlannedItem {
  itemId: string;
  assetId: string;
  kind: AssetKind;
  mediaPath: string;
  thumbnailPath?: string | null;
  trimStart: number;
  duration: number;
  fit: Fit;
}

export interface Plan {
  items: PlannedItem[];
  totalDuration: number;
}

export interface Snapshot {
  dir: string;
  project: Project;
  plan: Plan;
}

export interface RecentProject {
  name: string;
  path: string;
  updatedAt: number;
}

export interface EngineStatus {
  source: string;
  path: string;
  version: string;
}

export interface SkippedFile {
  path: string;
  reason: string;
}

export interface ImportResult {
  snapshot: Snapshot;
  added: number;
  skipped: SkippedFile[];
}

export interface ProgressEvent {
  phase: string;
  index: number;
  total: number;
  label: string;
}

export interface ExportSummary {
  outPath: string;
  totalDuration: number;
  encoder: string;
}

export interface SettingsPatch {
  width?: number;
  height?: number;
  fps?: number;
  crf?: number;
  preset?: string;
  encoder?: "auto" | "cpu" | "gpu";
  background?: Background;
  defaultImageDuration?: number;
  transition?: Transition;
}

export function formatSeconds(value: number): string {
  const rounded = Math.round(value * 10) / 10;
  return Number.isInteger(rounded) ? `${rounded}` : rounded.toFixed(1);
}

export function formatClock(value: number): string {
  const total = Math.max(0, Math.round(value));
  const minutes = Math.floor(total / 60);
  const seconds = total % 60;
  return minutes > 0 ? `${minutes} min ${seconds} s` : `${seconds} s`;
}

export function fileBasename(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] ?? path;
}

/// Friendly name for a copied asset: strip the `<uuid>-` safety prefix.
export function assetDisplayName(asset: Asset): string {
  return fileBasename(asset.fileName)
    .replace(/^[0-9a-f]{32}-/, "")
    .replace(/\.[^.]+$/, "");
}
