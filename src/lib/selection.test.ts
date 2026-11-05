import { describe, expect, it } from "vitest";
import type { ThreadRow } from "./api";
import { addRange, all, flagActionFor, keyOf, pick, prune, readActionFor, toggle } from "./selection";

const row = (id: string, extra: Partial<ThreadRow> = {}) => ({ accountId: 1, conversationId: id, unreadCount: 0, isFlagged: false, ...extra }) as ThreadRow;
const rows = [row("a"), row("b"), row("draft:1", { draftId: 1 }), row("c"), row("d")];

describe("selection", () => {
  it("toggles a row in and out", () => {
    const once = toggle(new Set(), rows[0]);
    expect([...once]).toEqual(["1:a"]);
    expect(toggle(once, rows[0]).size).toBe(0);
  });

  it("tells accounts apart", () => {
    expect(keyOf({ accountId: 2, conversationId: "a" })).not.toBe(keyOf(rows[0]));
  });

  it("adds a range in either direction, skipping drafts", () => {
    expect([...addRange(new Set(), rows, 3, 1)]).toEqual(["1:b", "1:c"]);
    expect([...addRange(new Set(["1:a"]), rows, 0, 99)]).toEqual(["1:a", "1:b", "1:c", "1:d"]);
  });

  it("selects every conversation but no drafts", () => {
    expect(all(rows).size).toBe(4);
    expect(all(rows).has("1:draft:1")).toBe(false);
  });

  it("drops rows that have left the list, keeping the same set when nothing changed", () => {
    const set = new Set(["1:a", "1:gone"]);
    expect([...prune(set, rows)]).toEqual(["1:a"]);
    const kept = new Set(["1:a"]);
    expect(prune(kept, rows)).toBe(kept);
  });

  it("picks the selected rows in list order", () => {
    expect(pick(new Set(["1:d", "1:a"]), rows).map((t) => t.conversationId)).toEqual(["a", "d"]);
  });

  it("u reads a mixed selection and only marks unread when it's all read", () => {
    expect(readActionFor([row("a"), row("b", { unreadCount: 1 })])).toBe("markRead");
    expect(readActionFor([row("a"), row("b")])).toBe("markUnread");
  });

  it("s flags a mixed selection and only unflags when it's all flagged", () => {
    expect(flagActionFor([row("a", { isFlagged: true }), row("b")])).toBe("flag");
    expect(flagActionFor([row("a", { isFlagged: true })])).toBe("unflag");
  });
});
