import { test, expect, row, snap, openSettings, showSettingsPage, waitForSyncIdle } from "./fixtures";
import type { Page } from "@playwright/test";
import { GMAIL_ACCOUNT, GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET, leaves, parseMime } from "../harness/fake-gmail.ts";

type Fake = Parameters<Parameters<typeof test>[2]>[0]["fake"];
const sentOfKind = (fake: Fake, kind: string) => fake.state.sent.filter((s: Record<string, any>) => s.kind === kind);
const ME = "boris@peetmail.test";
const sigRow = (page: Page, address = ME) => page.locator(`[data-testid=signature-row][data-address="${address}"]`);
/** The signature block in the composer's text. */
const sigBlock = (page: Page) => page.getByTestId("compose-body").locator(".peetmail_signature");
/** As sent: a line below the text, marked like Outlook's. */
const SENT_SIG = '<div id="Signature" class="peetmail_signature" style="margin-top:1em"><div><strong>Boris Dragon</strong><br>Peet Software';

/** Types a signature for `address` in Settings > Mail ("Boris Dragon" in bold, then a line). */
async function saveSignature(page: Page, opts: { replies?: boolean; managed?: boolean; address?: string } = {}) {
  await openSettings(page, "mail");
  const r = sigRow(page, opts.address);
  await r.getByTestId("rich-editor").click();
  await page.keyboard.press("Control+b");
  await page.keyboard.type("Boris Dragon");
  await page.keyboard.press("Control+b");
  await page.keyboard.press("Shift+Enter");
  await page.keyboard.type("Peet Software");
  if (opts.replies) await r.getByTestId("signature-replies").check();
  if (opts.managed) await r.getByTestId("signature-managed").check();
  await page.getByTestId("signature-save").click();
  await expect(page.getByTestId("signature-editor")).toContainText("Saved");
  await page.keyboard.press("Escape");
}

/** Types into the first line of the message, above the signature. */
async function typeBody(page: Page, text: string) {
  await page.getByTestId("compose-body").locator("> p").first().click();
  await page.keyboard.type(text);
}

test("a signature made in Settings goes under a new message, and can be edited there", async ({ inbox: page, fake }, testInfo) => {
  await saveSignature(page);
  // Kept after reopening Settings.
  await openSettings(page, "mail");
  await expect(sigRow(page).getByTestId("rich-editor")).toContainText("Boris Dragon");
  await page.keyboard.press("Escape");

  await page.keyboard.press("c");
  await expect(sigBlock(page)).toContainText("Boris DragonPeet Software");
  await page.getByTestId("compose-to").fill("priya@fabrikam.io");
  await page.getByTestId("compose-subject").fill("Lunch?");
  await typeBody(page, "Thai on Friday?");
  // Changed for this message only.
  await sigBlock(page).click();
  await page.keyboard.press("Control+End");
  await page.keyboard.type(" (on leave from Monday)");
  await expect(page.getByTestId("compose-body")).toHaveAttribute("data-text", "Thai on Friday?\n\n-- \nBoris Dragon\nPeet Software (on leave from Monday)");
  await snap(page, testInfo, "composer-signature");
  await page.keyboard.press("Control+Enter");

  await expect.poll(() => sentOfKind(fake, "new").length, { timeout: 10_000 }).toBe(1);
  expect(sentOfKind(fake, "new")[0].html).toContain(`<div>Thai on Friday?</div>${SENT_SIG} (on leave from Monday)</div></div>`);

  // The setting itself is unchanged.
  await page.keyboard.press("c");
  await expect(sigBlock(page)).not.toContainText("on leave");
});

test("a message with only its signature leaves no draft; a draft keeps the signature", async ({ inbox: page }) => {
  await saveSignature(page);
  await page.keyboard.press("c");
  await expect(sigBlock(page)).toBeVisible();
  await page.keyboard.press("Escape");
  await page.keyboard.press("g");
  await page.keyboard.press("d");
  await expect(page.getByTestId("drafts-empty")).toBeVisible();

  await page.keyboard.press("c");
  await typeBody(page, "half a thought");
  await expect(page.getByTestId("compose-hint")).toHaveText("Draft saved");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("draft-row")).toHaveCount(1);
  await page.keyboard.press("Enter");
  await expect(sigBlock(page)).toContainText("Boris Dragon");
  await expect(page.getByTestId("compose-body")).toContainText("half a thought");
});

test("Ctrl+Shift+S takes the signature off one message", async ({ inbox: page, fake }) => {
  await saveSignature(page);
  await page.keyboard.press("c");
  await page.getByTestId("compose-to").fill("priya@fabrikam.io");
  await typeBody(page, "No signature here");
  await page.keyboard.press("Control+Shift+S");
  await expect(sigBlock(page)).toHaveCount(0);
  await expect(page.getByTestId("compose-signature-add")).toBeVisible();
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOfKind(fake, "new").length, { timeout: 10_000 }).toBe(1);
  expect(sentOfKind(fake, "new")[0].html).not.toContain("Signature");
});

test("replies get the signature only when chosen, and it can be added to one", async ({ inbox: page, fake }) => {
  await saveSignature(page);
  await row(page, "conv-plain").click();
  await page.keyboard.press("Enter");
  await page.keyboard.press("r");
  await expect(page.getByTestId("compose-body")).toBeFocused();
  await expect(sigBlock(page)).toHaveCount(0);
  await page.keyboard.type("Sunday works!");
  await page.getByTestId("compose-signature-add").click();
  await expect(sigBlock(page)).toContainText("Boris Dragon");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOfKind(fake, "reply").length, { timeout: 10_000 }).toBe(1);
  // In the comment, which Graph puts above the quoted original.
  const html: string = sentOfKind(fake, "reply")[0].html;
  expect(html).toContain(`<div>Sunday works!</div>${SENT_SIG}</div></div>`);
  expect(html.indexOf("<strong>Boris Dragon</strong>")).toBeLessThan(html.indexOf("<blockquote>"));
  // The fake can have the reply before the composer closes; until then it has the keys and
  // Ctrl+, does nothing.
  await expect(page.getByTestId("composer")).toHaveCount(0);

  // With "replies and forwards too", it's there from the start.
  await openSettings(page, "mail");
  await sigRow(page).getByTestId("signature-replies").check();
  await page.getByTestId("signature-save").click();
  await expect(page.getByTestId("signature-editor")).toContainText("Saved");
  await page.keyboard.press("Escape");
  await page.keyboard.press("f");
  await expect(sigBlock(page)).toContainText("Boris Dragon");
  await typeBody(page, "FYI");
  await expect(page.getByTestId("compose-body")).toHaveAttribute("data-text", "FYI\n\n-- \nBoris Dragon\nPeet Software");
});

test("an account whose organisation adds signatures (CodeTwo) gets none from PeetMail", async ({ inbox: page, fake }) => {
  await saveSignature(page, { managed: true });
  await page.keyboard.press("c");
  await expect(sigBlock(page)).toHaveCount(0);
  await expect(page.getByTestId("compose-signature-managed")).toContainText("Your organisation adds your signature");
  await expect(page.getByTestId("compose-signature-add")).toHaveCount(0);
  await page.getByTestId("compose-to").fill("priya@fabrikam.io");
  await typeBody(page, "Hello");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOfKind(fake, "new").length, { timeout: 10_000 }).toBe(1);
  expect(sentOfKind(fake, "new")[0].html).not.toContain("Boris Dragon");
});

test.describe("with a Gmail account", () => {
  test.use({ withGmail: true });

  async function addGmail(page: Page) {
    await openSettings(page, "apps");
    await page.getByTestId("google-client-id").fill(GOOGLE_CLIENT_ID);
    await page.getByTestId("google-client-secret").fill(GOOGLE_CLIENT_SECRET);
    await page.getByTestId("google-client-save").click();
    await showSettingsPage(page, "accounts");
    await page.getByTestId("account-add-google").click();
    await expect(page.locator(`[data-testid=account-row][data-email="${GMAIL_ACCOUNT.email}"]`)).toBeVisible({ timeout: 15_000 });
    await page.keyboard.press("Escape");
    await waitForSyncIdle(page);
    await expect(row(page, "g-thread-trip")).toBeVisible({ timeout: 15_000 });
  }

  test("each account has its own signature; switching From swaps it, and Gmail gets a plain-text one too", async ({ inbox: page, gmail }) => {
    await addGmail(page);
    await saveSignature(page, { address: GMAIL_ACCOUNT.email.toLowerCase() });

    await page.keyboard.press("c");
    // The Microsoft account has none.
    await expect(sigBlock(page)).toHaveCount(0);
    await page.getByTestId("compose-from").selectOption({ label: `${GMAIL_ACCOUNT.name} <${GMAIL_ACCOUNT.email}>` });
    await expect(sigBlock(page)).toContainText("Boris Dragon");
    await page.getByTestId("compose-to").fill("kim@friends.example");
    await page.getByTestId("compose-subject").fill("Trip");
    await typeBody(page, "Shall we?");
    await page.keyboard.press("Control+Enter");

    await expect.poll(() => gmail.state.sent.filter((s) => s.kind === "send").length, { timeout: 15_000 }).toBe(1);
    const mime = parseMime(gmail.state.sent.filter((s) => s.kind === "send")[0].raw);
    const part = (type: string) => leaves(mime).find((l) => l.contentType === type)?.body.toString("utf8") ?? "";
    expect(part("text/plain").replace(/\r\n/g, "\n")).toBe("Shall we?\n\n-- \nBoris Dragon\nPeet Software");
    expect(part("text/html")).toContain(SENT_SIG);
  });
});
