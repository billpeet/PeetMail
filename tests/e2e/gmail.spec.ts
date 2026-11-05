import { readFile } from "node:fs/promises";
import { join } from "node:path";
import { test, expect, folderConversations, messageBody, row, selectedRow, selectWithKeys, snap, waitForSyncIdle, openSettings, showSettingsPage } from "./fixtures";
import type { Page } from "@playwright/test";
import { GMAIL_ACCOUNT, GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET } from "../harness/fake-gmail.ts";

test.use({ withGmail: true });

type Gmail = Parameters<Parameters<typeof test>[2]>[0]["gmail"];

async function setGoogleClient(page: Page, secret = GOOGLE_CLIENT_SECRET) {
  await page.getByTestId("google-client-id").fill(GOOGLE_CLIENT_ID);
  await page.getByTestId("google-client-secret").fill(secret);
  await page.getByTestId("google-client-save").click();
}

/** Signed in to Microsoft already (the inbox fixture); adds the Gmail account. */
async function addGmail(page: Page) {
  await openSettings(page, "apps");
  await setGoogleClient(page);
  await showSettingsPage(page, "accounts");
  await expect(page.getByTestId("account-add-google")).toBeEnabled();
  await page.getByTestId("account-add-google").click();
  await expect(page.locator(`[data-testid=account-row][data-email="${GMAIL_ACCOUNT.email}"]`)).toBeVisible({ timeout: 15_000 });
  await page.keyboard.press("Escape");
  await waitForSyncIdle(page);
  await expect(row(page, "g-thread-trip")).toBeVisible({ timeout: 15_000 });
}

/** Ctrl+R and wait for a Gmail history request and idle. */
async function syncGmail(page: Page, gmail: Gmail) {
  const before = gmail.state.requests.length;
  await page.keyboard.press("Control+r");
  await expect.poll(() => gmail.state.requests.slice(before).some((r) => r.path.endsWith("/history") || r.path.endsWith("/profile"))).toBe(true);
  await waitForSyncIdle(page);
}

const labelsOf = async (gmail: Gmail, id: string) => (await gmail.control(`message?id=${id}`)).labelIds as string[];

test("a Gmail account joins the inbox: inbox mail in the cache window, promotions under Other", async ({ inbox: page }, testInfo) => {
  await addGmail(page);
  await expect(page.locator(`[data-testid=account-row]`)).toHaveCount(0); // settings closed
  for (const t of ["g-thread-trip", "g-thread-photos", "g-thread-latin"]) await expect(row(page, t)).toBeVisible();
  await expect(row(page, "g-thread-sale")).toHaveCount(0); // CATEGORY_PROMOTIONS -> Other
  await expect(row(page, "g-thread-old-archived")).toHaveCount(0); // not in the inbox
  await expect(row(page, "g-thread-ancient")).toHaveCount(0); // older than the cache window
  await expect(row(page, "g-thread-trip")).toContainText("Lee Wong");
  await expect(row(page, "g-thread-trip")).toContainText("2"); // two inbox messages
  await snap(page, testInfo, "gmail-inbox");

  await page.keyboard.press("/");
  await page.keyboard.type("account:gmail sale");
  await expect(row(page, "g-thread-sale")).toContainText("Winter sale ❄"); // encoded-word subject
  await expect(row(page, "g-thread-sale")).toContainText("Gear Shop");
});

test("bodies, inline images, attachments and Latin-1 text come from the MIME parts", async ({ inbox: page, app }) => {
  await addGmail(page);
  await row(page, "g-thread-photos").click();
  await expect(messageBody(page).locator("p")).toContainText("Here they are");
  await expect(messageBody(page).locator("img")).toHaveAttribute("src", /^data:image\/png;base64,/);
  const chip = page.getByTestId("attachment");
  await expect(chip).toHaveCount(1); // the inline pixel isn't listed
  await expect(chip).toContainText("plan.pdf");
  await chip.click();
  await expect(page.getByTestId("toast")).toContainText("Saved plan.pdf");
  expect(await readFile(join(app.dataDir, "downloads", "plan.pdf"), "utf8")).toBe("%PDF-1.4 fake plan");

  await page.keyboard.press("Escape");
  await row(page, "g-thread-latin").click();
  await expect(messageBody(page).locator("p")).toHaveText("Café confirmado");

  // A thread includes your sent reply.
  await page.keyboard.press("Escape");
  await row(page, "g-thread-trip").click();
  await expect(page.getByTestId("message")).toHaveCount(3);
});

test("later syncs follow Gmail's history: new mail, read elsewhere, archived elsewhere, deleted", async ({ inbox: page, gmail }) => {
  await addGmail(page);
  await gmail.control("deliver", { subject: "Dinner Thursday?", from: "Sam Lee <sam@friends.example>" });
  await gmail.control("label", { id: "g-1", remove: ["UNREAD"] }); // read on the phone (g-2 is newer and read too)
  await gmail.control("label", { id: "g-4", remove: ["INBOX"] }); // archived on the phone
  await gmail.control("delete", { id: "g-5" });
  await syncGmail(page, gmail);
  await expect(page.locator("[data-testid=thread-row]", { hasText: "Dinner Thursday?" })).toBeVisible();
  await expect(row(page, "g-thread-trip")).toHaveAttribute("data-unread", "false");
  await expect(row(page, "g-thread-photos")).toHaveCount(0);
  await expect(row(page, "g-thread-latin")).toHaveCount(0);
  // Only metadata for what changed, not a full relist.
  expect(gmail.state.requests.filter((r) => r.path.endsWith("/messages") && r.method === "GET").length).toBe(4); // the first sync's lists (inbox, sent, archive and trash, spam)
  expect(await folderConversations(page, "archive", GMAIL_ACCOUNT.email)).toContain("g-thread-photos"); // archived on the phone: Done

  // History too old: start over, and pick up what changed.
  await gmail.control("expire-history", {});
  await gmail.control("deliver", { subject: "After the gap" });
  await syncGmail(page, gmail);
  await expect(page.locator("[data-testid=thread-row]", { hasText: "After the gap" })).toBeVisible();
});

test("archived, trashed and spam Gmail mail in the window is kept; old mail isn't", async ({ inbox: page, gmail }) => {
  await addGmail(page);
  const email = GMAIL_ACCOUNT.email;
  await expect.poll(() => folderConversations(page, "archive", email)).toEqual(["g-thread-old-archived"]);
  expect(await folderConversations(page, "deleteditems", email)).toEqual(["g-thread-trashed"]);
  expect(await folderConversations(page, "junkemail", email)).toEqual(["g-thread-spam"]);
  const everywhere = (await Promise.all(["inbox", "sentitems", "archive", "deleteditems", "junkemail"].map((r) => folderConversations(page, r, email)))).flat();
  expect(everywhere).not.toContain("g-thread-ancient");

  // Mail that arrives already archived (a filter skipping the inbox) lands in Done, not nowhere.
  await gmail.control("deliver", { id: "g-filtered", threadId: "g-thread-filtered", labelIds: ["CATEGORY_UPDATES"], subject: "Receipt" });
  await syncGmail(page, gmail);
  expect(await folderConversations(page, "archive", email)).toContain("g-thread-filtered");
});

test("spam is Gmail's SPAM label: report, not spam, and undoing a delete from Junk", async ({ inbox: page, gmail }) => {
  await addGmail(page);
  // Not spam: SPAM off, INBOX on.
  await page.keyboard.press("g");
  await page.keyboard.press("!");
  await expect(row(page, "g-thread-spam")).toBeVisible();
  await row(page, "g-thread-spam").click();
  await page.keyboard.press("!");
  await expect(row(page, "g-thread-spam")).toHaveCount(0);
  await expect.poll(() => labelsOf(gmail, "g-11"), { timeout: 10_000 }).toContain("INBOX");
  expect(await labelsOf(gmail, "g-11")).not.toContain("SPAM");

  // Report spam from the inbox: SPAM on, INBOX off; it shows in Junk.
  await page.keyboard.press("g");
  await page.keyboard.press("i");
  await selectWithKeys(page, "g-thread-latin");
  await page.keyboard.press("!");
  await expect(row(page, "g-thread-latin")).toHaveCount(0);
  await expect.poll(() => labelsOf(gmail, "g-5"), { timeout: 10_000 }).toContain("SPAM");
  expect(await labelsOf(gmail, "g-5")).not.toContain("INBOX");
  await syncGmail(page, gmail);
  await page.keyboard.press("g");
  await page.keyboard.press("!");
  await expect(row(page, "g-thread-latin")).toBeVisible();

  // # in Junk deletes; undoing once it reached Gmail puts it back in Spam, not the inbox.
  await row(page, "g-thread-latin").click();
  await page.keyboard.press("#");
  await expect.poll(() => labelsOf(gmail, "g-5"), { timeout: 10_000 }).toContain("TRASH");
  await page.keyboard.press("z");
  await expect.poll(() => labelsOf(gmail, "g-5"), { timeout: 10_000 }).not.toContain("TRASH");
  expect(await labelsOf(gmail, "g-5")).toContain("SPAM");
  expect(await labelsOf(gmail, "g-5")).not.toContain("INBOX");
});

test("Done and Trash are label changes too: back to the inbox, restore, and undoing a delete from Done", async ({ inbox: page, gmail }) => {
  await addGmail(page);
  // e in Done: back to the inbox.
  await page.keyboard.press("g");
  await page.keyboard.press("e");
  await expect(row(page, "g-thread-old-archived")).toBeVisible();
  await row(page, "g-thread-old-archived").click();
  await page.keyboard.press("e");
  await expect(row(page, "g-thread-old-archived")).toHaveCount(0);
  await expect.poll(() => labelsOf(gmail, "g-7"), { timeout: 10_000 }).toContain("INBOX");

  // # in Trash: out of Trash, into the inbox.
  await page.keyboard.press("g");
  await page.keyboard.press("#");
  await expect(row(page, "g-thread-trashed")).toBeVisible();
  await row(page, "g-thread-trashed").click();
  await page.keyboard.press("#");
  await expect.poll(() => labelsOf(gmail, "g-10"), { timeout: 10_000 }).toContain("INBOX");
  expect(await labelsOf(gmail, "g-10")).not.toContain("TRASH");

  // Deleting archived mail, then undoing once it reached Gmail: back in Done (not Trash, not the inbox).
  await gmail.control("label", { id: "g-7", remove: ["INBOX"] });
  await syncGmail(page, gmail);
  await page.keyboard.press("g");
  await page.keyboard.press("e");
  await row(page, "g-thread-old-archived").click();
  await page.keyboard.press("#");
  await expect.poll(() => labelsOf(gmail, "g-7"), { timeout: 10_000 }).toContain("TRASH");
  await page.keyboard.press("z");
  await page.keyboard.press("Escape"); // the reader moved on to the next conversation
  await expect(row(page, "g-thread-old-archived")).toBeVisible();
  await expect.poll(() => labelsOf(gmail, "g-7"), { timeout: 10_000 }).not.toContain("TRASH");
  expect(await labelsOf(gmail, "g-7")).not.toContain("INBOX");
});

test("triage becomes label changes: archive, read, star, delete, snooze, and undo", async ({ inbox: page, gmail }) => {
  await addGmail(page);
  await selectWithKeys(page, "g-thread-latin");
  await expect(selectedRow(page)).toHaveAttribute("data-conversation-id", "g-thread-latin");

  // e: archive = remove INBOX
  await page.keyboard.press("e");
  await expect(row(page, "g-thread-latin")).toHaveCount(0);
  await expect.poll(() => labelsOf(gmail, "g-5"), { timeout: 10_000 }).not.toContain("INBOX");

  // z: undo after it reached the server puts INBOX back
  await page.keyboard.press("z");
  await expect(row(page, "g-thread-latin")).toBeVisible();
  await expect.poll(() => labelsOf(gmail, "g-5"), { timeout: 10_000 }).toContain("INBOX");

  // u / s on the photos thread: read, starred
  await row(page, "g-thread-photos").click();
  await expect(page.getByTestId("reader")).toHaveAttribute("data-conversation-id", "g-thread-photos");
  await page.keyboard.press("s");
  await expect.poll(() => labelsOf(gmail, "g-4"), { timeout: 10_000 }).toEqual(expect.arrayContaining(["STARRED"]));
  expect(await labelsOf(gmail, "g-4")).not.toContain("UNREAD"); // opening read it

  // #: delete = trash
  await page.keyboard.press("#");
  await expect.poll(() => labelsOf(gmail, "g-4"), { timeout: 10_000 }).toContain("TRASH");

  // h: snooze = our hidden label, out of the inbox
  await page.keyboard.press("Escape");
  await selectWithKeys(page, "g-thread-trip");
  await expect(selectedRow(page)).toHaveAttribute("data-conversation-id", "g-thread-trip");
  await page.keyboard.press("h");
  await page.getByTestId("snooze-preset").and(page.locator("[data-id=tomorrow]")).click();
  await expect(row(page, "g-thread-trip")).toHaveCount(0);
  await expect.poll(() => gmail.state.sent.filter((s) => s.kind === "create-label")).toEqual([expect.objectContaining({ name: "PeetMail/Snoozed", labelListVisibility: "labelHide" })]);
  await expect.poll(async () => (await labelsOf(gmail, "g-2")).join(","), { timeout: 10_000 }).toBe("CATEGORY_PERSONAL,Label_1");
  // A sync sees the label change and keeps it snoozed.
  await syncGmail(page, gmail);
  await expect(row(page, "g-thread-trip")).toHaveCount(0);
});

test("Gmail-only setup: no Microsoft client needed", async ({ page }) => {
  await expect(page.getByTestId("setup")).toBeVisible();
  await expect(page.getByTestId("google-client-form")).toBeVisible();
  await setGoogleClient(page);
  await page.getByTestId("sign-in-google").click();
  await expect(row(page, "g-thread-trip")).toBeVisible({ timeout: 20_000 });
  await expect(page.getByTestId("footer-accounts")).toHaveText(GMAIL_ACCOUNT.email);
});

test("a missing client secret is reported when Google insists on one", async ({ inbox: page }) => {
  await openSettings(page, "apps");
  await setGoogleClient(page, "");
  await showSettingsPage(page, "accounts");
  await page.getByTestId("account-add-google").click();
  await expect(page.getByTestId("account-error")).toContainText("client_secret is missing", { timeout: 15_000 });
});

test("snippets with zero-width characters after '&' don't take the app down", async ({ inbox: page, gmail }) => {
  await addGmail(page);
  // Marketing mail pads previews with zero-width non-joiners. Here byte 10 after '&' falls
  // inside one, which is what crashed sync (the old code sliced 10 bytes ahead).
  await gmail.control("deliver", { subject: "Padded preview", snippet: "Sale &a\u200c\u200c\u200c now on" });
  await syncGmail(page, gmail);
  await expect(page.locator("[data-testid=thread-row]", { hasText: "Padded preview" })).toContainText("Sale &");
  await page.keyboard.press("Control+r"); // still alive and syncing
  await waitForSyncIdle(page);
});
