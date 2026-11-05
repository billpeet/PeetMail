// Several conversations selected at once (x, Shift+J/K, Ctrl+A), apart from the cursor.
// Kept as keys, so a selection survives the list reloading underneath it.

import type { Action, ThreadRow } from "./api";

type Ref = Pick<ThreadRow, "accountId" | "conversationId">;

export const keyOf = (t: Ref) => `${t.accountId}:${t.conversationId}`;

/** Drafts shown among conversations can't be selected. */
const selectable = (t: ThreadRow) => t.draftId === undefined;

/** Adds `t`, or takes it out if it's already in. */
export function toggle(set: ReadonlySet<string>, t: Ref): Set<string> {
  const next = new Set(set);
  const k = keyOf(t);
  if (!next.delete(k)) next.add(k);
  return next;
}

/** Adds every selectable row from index `a` to `b` (either order, both included). */
export function addRange(set: ReadonlySet<string>, rows: readonly ThreadRow[], a: number, b: number): Set<string> {
  const next = new Set(set);
  const [lo, hi] = a <= b ? [a, b] : [b, a];
  for (let i = Math.max(0, lo); i <= Math.min(hi, rows.length - 1); i++) {
    if (selectable(rows[i])) next.add(keyOf(rows[i]));
  }
  return next;
}

/** Every selectable row. */
export function all(rows: readonly ThreadRow[]): Set<string> {
  return new Set(rows.filter(selectable).map(keyOf));
}

/** The selection without rows that have left the list (archived elsewhere, synced away). */
export function prune(set: ReadonlySet<string>, rows: readonly ThreadRow[]): Set<string> {
  if (!set.size) return set as Set<string>;
  const present = new Set(rows.map(keyOf));
  const next = new Set([...set].filter((k) => present.has(k)));
  return next.size === set.size ? (set as Set<string>) : next;
}

/** The selected rows, in list order. */
export function pick(set: ReadonlySet<string>, rows: readonly ThreadRow[]): ThreadRow[] {
  return set.size ? rows.filter((t) => set.has(keyOf(t))) : [];
}

/** u on a selection: anything unread makes it all read, else all unread (like Gmail). */
export function readActionFor(rows: readonly ThreadRow[]): Action {
  return rows.some((t) => t.unreadCount > 0) ? "markRead" : "markUnread";
}

/** s on a selection: anything unflagged flags it all, else it's all unflagged. */
export function flagActionFor(rows: readonly ThreadRow[]): Action {
  return rows.some((t) => !t.isFlagged) ? "flag" : "unflag";
}
