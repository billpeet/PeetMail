import { test, expect, row, selectedRow } from "./fixtures";
import type { Page } from "@playwright/test";

type Fake = Parameters<Parameters<typeof test>[2]>[0]["fake"];

const folderOf = async (fake: Fake, id: string) => (await fake.control(`message?id=${id}`)).parentFolderId as string;
const rows = (page: Page) => page.getByTestId("thread-row");
const checkedRows = (page: Page) => page.locator("[data-testid=thread-row][data-checked=true]");
const ids = (page: Page) => rows(page).evaluateAll((els) => els.map((e) => e.getAttribute("data-conversation-id")!));

test("x and Shift+J select several conversations, e archives them all, one z brings them all back", async ({ inbox: page, fake }, testInfo) => {
  const [first, second, third] = await ids(page);
  expect(first).toBe("conv-hostile");

  await page.keyboard.press("x");
  await expect(row(page, first)).toHaveAttribute("data-checked", "true");
  await expect(selectedRow(page)).toHaveAttribute("data-conversation-id", first); // x doesn't move the cursor
  await page.keyboard.press("Shift+J");
  await page.keyboard.press("Shift+J");
  await expect(selectedRow(page)).toHaveAttribute("data-conversation-id", third);
  await expect(checkedRows(page)).toHaveCount(3);
  await expect(page.getByTestId("selection-count")).toContainText("3 selected");
  await page.screenshot({ path: testInfo.outputPath("selected.png") });

  await page.keyboard.press("e");
  for (const id of [first, second, third]) await expect(row(page, id)).toHaveCount(0);
  await expect(page.getByTestId("toast")).toContainText("Archived 3 conversations");
  await expect(page.getByTestId("selection-count")).toHaveCount(0);
  await expect.poll(() => folderOf(fake, "msg-hostile"), { timeout: 10_000 }).toBe("AAMkAGI2-archive");

  await page.keyboard.press("z");
  await expect(page.getByTestId("toast")).toContainText("Undid: archived 3 conversations");
  for (const id of [first, second, third]) await expect(row(page, id)).toHaveCount(1);
  await expect.poll(() => folderOf(fake, "msg-hostile"), { timeout: 10_000 }).toBe("AAMkAGI2-inbox");
});

test("x takes a conversation back out, and Esc clears the selection", async ({ inbox: page }) => {
  await page.keyboard.press("Shift+J");
  await expect(checkedRows(page)).toHaveCount(2);
  await page.keyboard.press("x"); // the cursor's on the second one
  await expect(checkedRows(page)).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(checkedRows(page)).toHaveCount(0);
  await expect(page.getByTestId("selection-count")).toHaveCount(0);
});

test("Ctrl+A selects the whole view, rows out of sight too, and u marks every conversation read", async ({ inbox: page }) => {
  // The list only renders the rows in view; the header has the real count.
  const total = Number((await page.getByTestId("thread-count").textContent())!.match(/\d+/)![0]);
  expect(total).toBeGreaterThan(await rows(page).count());
  await expect(page.locator("[data-testid=thread-row][data-unread=true]").first()).toBeVisible();
  await page.keyboard.press("Control+a");
  await expect(page.getByTestId("selection-count")).toContainText(`${total} selected`);
  await expect(checkedRows(page)).toHaveCount(await rows(page).count());

  await page.keyboard.press("u");
  await expect(page.getByTestId("toast")).toContainText(`Marked as read`);
  await expect(page.locator("[data-testid=thread-row][data-unread=true]")).toHaveCount(0);
  await page.keyboard.press("G"); // the last row was off screen when it was marked
  await expect(selectedRow(page)).toHaveAttribute("data-unread", "false");
  // Read and flag keep the selection, so another key can follow.
  await expect(page.getByTestId("selection-count")).toContainText(`${total} selected`);
});

test("Ctrl+A in the search box selects its text, not the conversations", async ({ inbox: page }) => {
  await page.keyboard.press("/");
  await page.getByTestId("search-input").fill("budget");
  await page.keyboard.press("Control+a");
  await expect(page.getByTestId("selection-count")).toHaveCount(0);
  expect(await page.getByTestId("search-input").evaluate((el: HTMLInputElement) => el.selectionEnd! - el.selectionStart!)).toBe(6);
});

test("a selection snoozes together and lands in Snoozed", async ({ inbox: page }) => {
  const [first, second] = await ids(page);
  await page.keyboard.press("Shift+J");
  await page.keyboard.press("h");
  await expect(page.getByTestId("snooze-target")).toHaveText("2 conversations");
  await page.keyboard.press("Enter");
  await expect(row(page, first)).toHaveCount(0);
  await expect(row(page, second)).toHaveCount(0);
  await expect(page.getByTestId("toast")).toContainText("Snoozed 2 conversations until");

  await page.keyboard.press("g");
  await page.keyboard.press("h");
  await expect(row(page, first)).toHaveCount(1);
  await expect(row(page, second)).toHaveCount(1);
});

test("Ctrl+click and Shift+click select rows without opening them", async ({ inbox: page }) => {
  const [first, , third] = await ids(page);
  await row(page, first).click({ modifiers: ["Control"] });
  await expect(row(page, first)).toHaveAttribute("data-checked", "true");
  await expect(page.getByTestId("reader")).toHaveCount(0);
  await row(page, third).click({ modifiers: ["Shift"] });
  await expect(checkedRows(page)).toHaveCount(3);
  await expect(selectedRow(page)).toHaveAttribute("data-conversation-id", third);
});

test("in the preview layout the reading pane sums up the selection", async ({ inbox: page }) => {
  await page.keyboard.press("v");
  await expect(page.getByTestId("main")).toHaveAttribute("data-layout", "preview");
  await page.keyboard.press("Shift+J");
  await expect(page.getByTestId("selection-summary")).toContainText("2 conversations selected");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("selection-summary")).toHaveCount(0);
  await expect(page.getByTestId("reader")).toBeVisible();
});
