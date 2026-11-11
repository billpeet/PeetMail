// Two layouts from one set of components (docs/ANDROID.md, Stage 2):
//
//   desktop  today's: rail, list beside the reading pane, dialogs over the window
//   compact  a phone: one screen at a time, a bottom bar, dialogs as full-screen pages
//
// Compact on Android, and on any window 640px wide or less, so the E2E suite can drive it
// on Windows by shrinking the viewport. Components read `layout.compact`, never the user
// agent.

import { MediaQuery } from "svelte/reactivity";
import { platform, type Platform } from "./platform";

export const COMPACT_MAX_WIDTH = 640;

/** A phone is always compact; a desktop is when its window is narrow. */
export function isCompact(p: Platform, narrow: boolean): boolean {
  return p === "android" || narrow;
}

const narrow = new MediaQuery(`max-width: ${COMPACT_MAX_WIDTH}px`, false);

export const layout = {
  get compact(): boolean {
    return isCompact(platform, narrow.current);
  },
};
