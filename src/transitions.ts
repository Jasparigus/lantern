// The one place the frontend does transition math. Mirrors the Rust exporter
// (`TRANSITION_SECONDS` / `transition_blend` in src-tauri/src/export.rs) so the
// durations shown on screen match the video that gets created.

import type { PlannedItem, Transition } from "./types";

/// Longest blend the exporter will attempt between two adjacent items.
export const TRANSITION_SECONDS = 0.75;

/// Seconds the exporter actually blends each adjacent pair.
///
/// `durations` are the full per-item segment lengths straight from the plan
/// (photos: their duration, videos: their trimmed length) — the plan never
/// accounts for transitions itself, so this is where the shrink is applied.
export function transitionBlendSeconds(
  transition: Transition,
  durations: number[],
  fps: number,
): number {
  const count = durations.length;
  // No blend without neighbours, and only the overlapping styles blend.
  switch (transition) {
    case "crossfade":
    case "slide":
    case "checkerboard":
      break;
    // "none", "kenburns", or anything unrecognized: plain cuts.
    default:
      return 0;
  }
  if (count < 2) return 0;
  const minDuration = Math.min(...durations);
  const room = count === 2 ? minDuration - 0.3 : (minDuration - 0.3) / 2;
  const blend = Math.min(TRANSITION_SECONDS, room);
  // Tiny clips can't spare enough time for a blend to read as a transition.
  if (blend < 0.2) return 0;
  // Snap to the output frame grid — the exporter emits whole frames and snaps
  // the blend the same way, so shown durations match the rendered file.
  return Math.round(blend * fps) / fps;
}

/// Whole-show length: the sum of the item segments minus every overlap the
/// transitions eat.
export function showDurationSeconds(
  transition: Transition,
  durations: number[],
  fps: number,
): number {
  const sum = durations.reduce((total, duration) => total + duration, 0);
  const blend = transitionBlendSeconds(transition, durations, fps);
  return Math.max(0, sum - Math.max(0, durations.length - 1) * blend);
}

/// Convenience for call sites that hold a plan.
export function planDurations(items: PlannedItem[]): number[] {
  return items.map((item) => item.duration);
}
