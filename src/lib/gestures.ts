// Touch gestures on the phone layout (docs/ANDROID.md, Stage 3): what a finger's movement
// means, kept apart from the components so it can be tested without one.

/** Movement before a touch counts as a drag (and stops being a tap or a long press). */
export const SLOP_PX = 10;
/** Held this long without moving: a long press. */
export const LONG_PRESS_MS = 450;
/** A pull at the top of the list this far (after damping) syncs. */
export const PULL_TRIGGER_PX = 64;

/** Which way a drag goes, once it's clearly one or the other; null while undecided. */
export function dragAxis(dx: number, dy: number): "x" | "y" | null {
  const ax = Math.abs(dx);
  const ay = Math.abs(dy);
  if (Math.max(ax, ay) < SLOP_PX) return null;
  return ax > ay * 1.5 ? "x" : "y";
}

/** How far a row must be dragged to act: under a third of it, at most 120px. */
export function rowThreshold(width: number): number {
  return Math.min(120, width * 0.3);
}

/** A row let go after a sideways drag: right archives, left snoozes, too short does nothing. */
export function rowSwipe(dx: number, width: number): "archive" | "snooze" | null {
  const t = rowThreshold(width);
  return dx >= t ? "archive" : dx <= -t ? "snooze" : null;
}

/** A flick across the conversation: left for the next one, right for the previous. */
export function pageSwipe(dx: number, dy: number, ms: number): "next" | "prev" | null {
  if (ms > 700 || Math.abs(dx) < 80 || Math.abs(dx) < Math.abs(dy) * 2) return null;
  return dx < 0 ? "next" : "prev";
}

/** How far the pull-to-refresh indicator moves for a finger pulled `dy` down: it resists. */
export function pullDistance(dy: number): number {
  if (dy <= 0) return 0;
  return Math.min(110, dy * 0.5);
}
