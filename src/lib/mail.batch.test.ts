import { describe, expect, it, vi } from "vitest";

// A big inbox behind a fake `list_threads` that, like the real one, returns at most 1000
// rows per query and answers asynchronously (so reloads and page loads can interleave).
const TOTAL = 2500;
const all = Array.from({ length: TOTAL }, (_, i) => ({
  accountId: 1,
  conversationId: `c${i}`,
  messageId: i + 1,
  sortAt: new Date(Date.UTC(2026, 9, 1) - i * 60_000).toISOString(),
  unreadCount: 0,
  isFlagged: false,
}));
const tick = () => new Promise((r) => setTimeout(r, 1));
vi.mock("./api", () => ({
  api: {
    listSplits: () => Promise.resolve([]),
    recentOpens: () => Promise.resolve([]),
    listDrafts: () => Promise.resolve([]),
    listThreads: async (_m: unknown, _q: unknown, _s: unknown, cursor: { messageId: number } | null, limit: number) => {
      await tick();
      const from = cursor ? all.findIndex((t) => t.messageId === cursor.messageId) + 1 : 0;
      return all.slice(from, from + Math.min(limit, 1000));
    },
  },
}));
vi.mock("./bodies", () => ({ prefetchBody: () => Promise.resolve() }));
vi.mock("./calendar.svelte", () => ({ calendar: { refresh: () => Promise.resolve() } }));
vi.mock("./accounts.svelte", () => ({ visibility: { among: () => [] } }));

const { mail } = await import("./mail.svelte");

async function freshInbox() {
  mail.boot = { accounts: [{ id: 1, provider: "microsoft", email: "a@b.c", displayName: null, signedIn: true }], preferences: {} } as never;
  mail.threads = [];
  mail.clearChecked();
  await mail.refresh();
  expect(mail.threads.length).toBe(200);
}

describe("Ctrl+A on a big view", () => {
  it("selects every conversation, and a reload afterwards keeps them all", async () => {
    await freshInbox();
    await mail.checkAll();
    expect(mail.checked.size).toBe(TOTAL);

    // A sync lands (or the action just taken): the reload must bring back all 2500 rows,
    // not the first 1000, or the selection beyond them would be dropped.
    await mail.refresh();
    expect(mail.threads.length).toBe(TOTAL);
    expect(mail.checked.size).toBe(TOTAL);
  });

  it("isn't stopped or left with gaps by reloads landing while it loads", async () => {
    await freshInbox();
    const selecting = mail.checkAll();
    for (let i = 0; i < 5; i++) {
      await tick();
      void mail.refresh();
    }
    await selecting;
    expect(mail.checked.size).toBe(TOTAL);
    expect(new Set(mail.threads.map((t) => t.conversationId)).size).toBe(mail.threads.length);
  });
});
