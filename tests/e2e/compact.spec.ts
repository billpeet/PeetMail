// The phone layout (docs/ANDROID.md, Stage 2), driven on Windows in a phone-sized window:
// one screen at a time, a bottom bar, the mailboxes in a drawer, dialogs as full pages.
// Escape stands in for Android's back button, which sends it (+page.svelte, androidBack).

import { test, expect, row, folderConversations, snap } from "./fixtures";

test.use({ phone: true });

test("the phone layout: no title bar or rail, a bottom bar, and a conversation opens full screen", async ({ inbox: page }, testInfo) => {
  await expect(page.getByTestId("compact-shell")).toBeVisible();
  await expect(page.getByTestId("titlebar")).toHaveCount(0);
  await expect(page.getByTestId("rail")).toHaveCount(0);
  await expect(page.getByTestId("bottom-nav")).toBeVisible();
  await expect(page.getByTestId("main")).toHaveAttribute("data-layout", "compact");
  await snap(page, testInfo, "phone-list");

  await row(page, "conv-plain").click();
  await expect(page.getByTestId("compact-shell")).toHaveAttribute("data-view", "reader");
  await expect(page.getByTestId("reader")).toHaveAttribute("data-conversation-id", "conv-plain");
  // The list and the bottom bar make way for the conversation and its own action bar.
  await expect(page.getByTestId("thread-list")).toHaveCount(0);
  await expect(page.getByTestId("bottom-nav")).toHaveCount(0);
  await expect(page.getByTestId("reader-actions")).toBeVisible();
  await snap(page, testInfo, "phone-reader");

  // Back (Esc, Android's back button) returns to the list.
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("thread-list")).toBeVisible();
  await expect(page.getByTestId("bottom-nav")).toBeVisible();
});

test("archive from the reader's bottom bar, then the toast's Undo button brings it back", async ({ inbox: page, fake }) => {
  await row(page, "conv-plain").click();
  await page.getByTestId("reader-archive").click();
  // As on desktop, the reader moves on to the next conversation.
  await expect(page.getByTestId("reader")).not.toHaveAttribute("data-conversation-id", "conv-plain");
  await expect(page.getByTestId("toast-undo")).toBeVisible();
  await page.getByTestId("toast-undo").click();
  await page.keyboard.press("Escape");
  await expect(row(page, "conv-plain")).toHaveCount(1);
  // Undone within the outbox's grace period: Graph never heard of it.
  await page.waitForTimeout(3000);
  expect(fake.state.sent.filter((s) => s.kind === "move")).toEqual([]);
});

test("the reader's more menu holds the rest: flag, then reply all opens the reply under the conversation", async ({ inbox: page }) => {
  await row(page, "conv-plain").click();
  await page.getByTestId("reader-more").click();
  await expect(page.getByTestId("reader-menu")).toBeVisible();
  await page.getByTestId("reader-flag").click();
  await expect(page.getByTestId("reader-menu")).toHaveCount(0);

  // Esc closes the menu first, without leaving the conversation.
  await page.getByTestId("reader-more").click();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("reader-menu")).toHaveCount(0);
  await expect(page.getByTestId("reader")).toBeVisible();

  await page.getByTestId("reader-more").click();
  await page.getByTestId("reader-reply-all").click();
  await expect(page.getByTestId("reader").getByTestId("composer")).toHaveAttribute("data-inline", "true");
});

test("the drawer goes to another mailbox; back from there returns to the inbox", async ({ inbox: page }) => {
  await page.getByTestId("drawer-open").click();
  await expect(page.getByTestId("drawer")).toBeVisible();
  await page.getByTestId("drawer-mailbox-sent").click();
  await expect(page.getByTestId("drawer")).toHaveCount(0);
  await expect(page.getByTestId("mailbox-title")).toHaveAttribute("data-mailbox", "sent");

  // Esc closes an open drawer before anything else.
  await page.getByTestId("drawer-open").click();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("drawer")).toHaveCount(0);
  await expect(page.getByTestId("mailbox-title")).toHaveAttribute("data-mailbox", "sent");
});

test("search from the bottom bar: the box takes the keyboard, results list, close ends it", async ({ inbox: page }) => {
  await page.getByTestId("nav-search").click();
  const input = page.getByTestId("search-input");
  await expect(input).toBeFocused();
  await input.pressSequentially("weekend");
  await expect(row(page, "conv-plain")).toBeVisible();
  await page.getByTestId("search-close").click();
  await expect(input).toHaveCount(0);
  await expect(page.getByTestId("mailbox-title")).toHaveAttribute("data-mailbox", "inbox");
});

test("compose from the floating button fills the screen and sends", async ({ inbox: page, fake }, testInfo) => {
  await page.getByTestId("compose-fab").click();
  const composer = page.getByTestId("composer");
  await expect(composer).toBeVisible();
  // A full page: as wide as the screen.
  const box = await composer.boundingBox();
  const width = await page.evaluate(() => innerWidth);
  expect(box!.width).toBeGreaterThanOrEqual(width - 1);
  await page.getByTestId("compose-to").fill("priya@fabrikam.io");
  await page.getByTestId("compose-subject").fill("From my phone");
  await page.getByTestId("compose-body").fill("Sent from PeetMail");
  await snap(page, testInfo, "phone-composer");
  // Send sits on screen, not cut off past the edge.
  const send = await page.getByTestId("compose-send").boundingBox();
  expect(send!.x + send!.width).toBeLessThanOrEqual(width);
  await page.getByTestId("compose-send").click();
  await expect(composer).toHaveCount(0);
  await expect.poll(() => fake.state.sent.filter((s) => s.kind === "new").length, { timeout: 10_000 }).toBe(1);
});

test("settings is a list of sections; a section opens on its own page and back returns to the list", async ({ inbox: page }) => {
  await page.getByTestId("nav-settings").click();
  const settings = page.getByTestId("settings");
  await expect(settings).toHaveAttribute("data-browsing", "true");
  await page.getByTestId("settings-nav-storage").click();
  await expect(settings).toHaveAttribute("data-browsing", "false");
  await expect(settings).toHaveAttribute("data-page", "storage");
  await expect(page.getByTestId("cache-save")).toBeVisible();

  // Back goes to the list first, then closes Settings.
  await page.keyboard.press("Escape");
  await expect(settings).toHaveAttribute("data-browsing", "true");
  await page.keyboard.press("Escape");
  await expect(settings).toHaveCount(0);

  await page.getByTestId("nav-settings").click();
  await page.getByTestId("settings-close").click();
  await expect(settings).toHaveCount(0);
});

test("the calendar shows the day, within the screen above the bottom bar", async ({ inbox: page }) => {
  await page.getByTestId("nav-calendar").click();
  await expect(page.getByTestId("mailbox-title")).toHaveAttribute("data-mailbox", "calendar");
  const day = page.getByTestId("day-view");
  await expect(day).toBeVisible();
  const dayBox = await day.boundingBox();
  const navBox = await page.getByTestId("bottom-nav").boundingBox();
  expect(dayBox!.y + dayBox!.height).toBeLessThanOrEqual(navBox!.y + 1);
  await page.getByTestId("nav-mail").click();
  await expect(page.getByTestId("thread-list")).toBeVisible();
});

test("mail laid out for a desktop screen is zoomed out to the phone's width", async ({ inbox: page, fake }) => {
  await fake.control("deliver", {
    subject: "Rigid newsletter",
    body: '<div style="min-width:700px"><div style="white-space:nowrap;font-size:28px">SALE: everything must go this weekend</div><p>Laid out for a 700 pixel wide screen.</p></div>',
  });
  await page.keyboard.press("Control+r");
  await page.getByTestId("thread-row").filter({ hasText: "Rigid newsletter" }).click();
  const body = page.getByTestId("message-body");
  await expect(body).toHaveAttribute("data-ready", "true");
  await expect(body).toHaveAttribute("data-scale", /^0\.\d+$/);
  const fits = await body.evaluate((f: HTMLIFrameElement) => {
    const root = f.contentDocument!.documentElement;
    return { scroll: root.scrollWidth, client: root.clientWidth, frame: f.clientHeight, content: root.scrollHeight };
  });
  // No sideways scroll, and the frame is as tall as the zoomed content (no gap below it).
  expect(fits.scroll).toBeLessThanOrEqual(fits.client + 1);
  expect(Math.abs(fits.frame - fits.content)).toBeLessThanOrEqual(2);

  // An ordinary message isn't touched.
  await page.keyboard.press("Escape");
  await row(page, "conv-plain").click();
  await expect(page.getByTestId("message-body")).toHaveAttribute("data-ready", "true");
  await expect(page.getByTestId("message-body")).not.toHaveAttribute("data-scale", /./);
});

test("archived from the reader lands in Done, as on desktop", async ({ inbox: page }) => {
  await row(page, "conv-plain").click();
  await page.getByTestId("reader-archive").click();
  await expect.poll(() => folderConversations(page, "archive"), { timeout: 10_000 }).toContain("conv-plain");
});
