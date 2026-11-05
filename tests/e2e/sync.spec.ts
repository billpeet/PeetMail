import { signIn } from "../harness/app.ts";
import { test, expect, DEFAULT_CONVERSATIONS, folderConversations, row, syncNow, waitForSyncIdle } from "./fixtures";

test("new mail appears after Ctrl+R", async ({ inbox: page, fake }) => {
  await fake.control("deliver", { subject: "Hot off the press", from: ["Priya Nair", "priya@fabrikam.io"] });
  await syncNow(page, fake);
  await expect(page.getByTestId("thread-row").first()).toContainText("Hot off the press");
  await expect(page.getByTestId("thread-count")).toHaveText(`${DEFAULT_CONVERSATIONS + 1} conversations`);
});

test("remote read-state changes and deletions sync through delta", async ({ inbox: page, fake }) => {
  await expect(row(page, "conv-hostile")).toHaveAttribute("data-unread", "true");
  await fake.control("update", { id: "msg-hostile", fields: { isRead: true } });
  await fake.control("remove", { id: "msg-plain" });
  await syncNow(page, fake);
  await expect(row(page, "conv-hostile")).toHaveAttribute("data-unread", "false");
  await expect(row(page, "conv-plain")).toHaveCount(0);
});

test("a sync with no changes costs one request per synced folder, the calendar and the drafts list", async ({ inbox: page, fake }) => {
  const before = fake.state.requests.length;
  await syncNow(page, fake);
  const reqs = fake.requestsSince(before);
  expect(reqs.map((r) => r.path)).toEqual([
    "/v1.0/me/mailFolders/AAMkAGI2-inbox/messages/delta",
    "/v1.0/me/mailFolders/AAMkAGI2-sent/messages/delta",
    "/v1.0/me/mailFolders/AAMkAGI2-archive/messages/delta",
    "/v1.0/me/mailFolders/AAMkAGI2-deleted/messages/delta",
    "/v1.0/me/mailFolders/AAMkAGI2-junk/messages/delta",
    "/v1.0/me/calendarView/delta",
    "/v1.0/me/mailFolders/drafts/messages",
  ]);
  expect(reqs.filter((r) => r.path.endsWith("/delta")).every((r) => r.query.includes("deltatoken"))).toBe(true);
});

test("archived and deleted mail syncs too, including what's moved there elsewhere", async ({ inbox: page, fake }) => {
  // Done before PeetMail, in another client.
  await expect.poll(() => folderConversations(page, "archive")).toContain("conv-archived");
  expect(await folderConversations(page, "deleteditems")).toContain("conv-deleted");
  await expect(row(page, "conv-archived")).toHaveCount(0); // not in the inbox

  // Archived and deleted on the phone.
  await fake.control("move", { id: "msg-plain", folder: "archive" });
  await fake.control("move", { id: "msg-hostile", folder: "deleteditems" });
  await syncNow(page, fake);
  await expect(row(page, "conv-plain")).toHaveCount(0);
  expect(await folderConversations(page, "archive")).toContain("conv-plain");
  expect(await folderConversations(page, "deleteditems")).toContain("conv-hostile");

  // And back to the inbox from Done.
  await fake.control("move", { id: "msg-plain", folder: "inbox" });
  await syncNow(page, fake);
  await expect(row(page, "conv-plain")).toBeVisible();
  expect(await folderConversations(page, "archive")).not.toContain("conv-plain");
});

test("mail archived in PeetMail stays in Done once the Archive folder syncs", async ({ inbox: page, fake }) => {
  await row(page, "conv-plain").click();
  await page.keyboard.press("e");
  await expect(row(page, "conv-plain")).toHaveCount(0);
  await expect.poll(() => fake.state.sent.some((s: any) => s.kind === "move"), { timeout: 10_000 }).toBe(true);
  await syncNow(page, fake);
  expect(await folderConversations(page, "archive")).toContain("conv-plain");
  expect(await folderConversations(page, "inbox")).not.toContain("conv-plain");
});

test("each mailbox lists its conversations, and Trash's and Junk's readers show their mail", async ({ inbox: page }) => {
  const invoke = (cmd: string, args: object) => page.evaluate(([c, a]) => (window as any).__TAURI_INTERNALS__.invoke(c, a), [cmd, args] as const);
  const convs = async (mailbox: string) =>
    ((await invoke("list_threads", { mailbox, query: "", limit: 1000 })) as { conversationId: string }[]).map((t) => t.conversationId);
  await expect.poll(() => convs("done")).toEqual(["conv-archived"]);
  expect(await convs("trash")).toEqual(["conv-deleted"]);
  expect(await convs("junk")).toEqual(["conv-junk"]);
  expect(await convs("sent")).toContain("conv-sent-0");
  expect(await convs("inbox")).not.toContain("conv-archived");
  const all = await convs("all");
  expect(all).toEqual(expect.arrayContaining(["conv-archived", "conv-plain", "conv-sent-0"]));
  expect(all).not.toContain("conv-deleted");
  expect(all).not.toContain("conv-junk");
  expect(await convs("inbox")).not.toContain("conv-junk");

  expect(await invoke("get_thread", { accountId: 1, conversationId: "conv-deleted" })).toEqual([]);
  expect(await invoke("get_thread", { accountId: 1, conversationId: "conv-deleted", from: "trash" })).toHaveLength(1);
  expect(await invoke("get_thread", { accountId: 1, conversationId: "conv-junk", from: "inbox" })).toEqual([]);
  expect(await invoke("get_thread", { accountId: 1, conversationId: "conv-junk", from: "junk" })).toHaveLength(1);
});

test("429 throttling is retried after Retry-After", async ({ inbox: page, fake }) => {
  await fake.control("throttle", { count: 1, retryAfter: 1 });
  await fake.control("deliver", { subject: "Worth the wait" });
  await syncNow(page, fake);
  await expect(page.getByTestId("thread-row").first()).toContainText("Worth the wait");
  expect(fake.state.requests.some((r) => r.status === 429)).toBe(true);
});

test("an expired delta link triggers a full resync", async ({ inbox: page, fake }) => {
  await fake.control("config", { expireDelta: true });
  await fake.control("deliver", { subject: "After the resync" });
  await syncNow(page, fake);
  await expect(page.getByTestId("thread-row").first()).toContainText("After the resync");
  expect(fake.state.requests.some((r) => r.status === 410)).toBe(true);
  await expect(page.getByTestId("thread-count")).toHaveText(`${DEFAULT_CONVERSATIONS + 1} conversations`);
});

test("a revoked access token is refreshed and the request retried", async ({ inbox: page, fake }) => {
  await fake.control("expire-tokens");
  await fake.control("deliver", { subject: "Fresh token" });
  const before = fake.state.requests.length;
  await syncNow(page, fake);
  await expect(page.getByTestId("thread-row").first()).toContainText("Fresh token");
  const seq = fake.requestsSince(before).map((r) => `${r.status} ${r.path}`);
  expect(seq).toContain("401 /v1.0/me/mailFolders/AAMkAGI2-inbox/messages/delta");
  expect(seq).toContain("200 /oauth/token");
  await expect(page.getByTestId("sync-status")).toHaveAttribute("data-state", "idle");
});

test("if Graph rejects the delta date filter, sync falls back to local filtering", async ({ page, fake }) => {
  await fake.control("config", { rejectDeltaFilter: true });
  await signIn(page, fake.clientId);
  await waitForSyncIdle(page);
  await expect(page.getByTestId("thread-count")).toHaveText(`${DEFAULT_CONVERSATIONS} conversations`);
  const inboxDeltas = fake.state.requests.filter((r) => r.path.endsWith("/AAMkAGI2-inbox/messages/delta"));
  expect(inboxDeltas[0].status).toBe(400);
  // The fallbacks drop the meeting-type field first, then the date filter.
  const firstOk = inboxDeltas.find((r) => r.status === 200)!;
  expect(firstOk.query).not.toContain("filter");
});
