import { test, expect, openSettings } from "./fixtures";
import { readFile } from "node:fs/promises";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { DOC_SLUGS, parseDoc } from "../../src/lib/docs.ts";
import { parseChangelog } from "../../src/lib/changelog.ts";

const title = (slug: string) => parseDoc(slug, readFileSync(`docs/user/${slug}.md`, "utf8")).title;

test("F1 opens Help at Getting started, with every doc page from docs/user", async ({ inbox: page }) => {
  await page.keyboard.press("F1");
  const help = page.getByTestId("help");
  await expect(help).toHaveAttribute("data-page", "getting-started");
  await expect(page.getByTestId("help-content")).toContainText("Install");
  // The Android page shows only on a phone until the Android app is released.
  await expect(page.getByTestId("help-nav-android")).toHaveCount(0);
  for (const slug of DOC_SLUGS.filter((s) => s !== "android")) {
    await expect(page.getByTestId(`help-nav-${slug}`)).toHaveText(title(slug));
    await page.getByTestId(`help-nav-${slug}`).click();
    await expect(page.getByTestId("help-content").locator("h2").first()).toHaveText(title(slug));
  }
  await page.keyboard.press("Escape");
  await expect(help).toHaveCount(0);
});

test("links between docs switch pages; web links open in the browser", async ({ inbox: page, app }) => {
  await page.keyboard.press("F1");
  const content = page.getByTestId("help-content");
  await content.getByRole("link", { name: "Updates" }).click();
  await expect(page.getByTestId("help")).toHaveAttribute("data-page", "updates");

  await page.getByTestId("help-nav-getting-started").click();
  await content.getByRole("link", { name: "releases page" }).click();
  await expect.poll(() => readFile(join(app.dataDir, "opened.txt"), "utf8").catch(() => "")).toContain("https://github.com/billpeet/peetmail/releases");
  // The app's window never navigated away.
  await expect(page.getByTestId("help")).toBeVisible();
});

test("the shortcuts and What's new pages come from the keymap and the changelog", async ({ inbox: page }) => {
  await page.keyboard.press("F1");
  await page.getByTestId("help-nav-shortcuts").click();
  await expect(page.getByTestId("shortcut-list")).toContainText("Archive");
  await expect(page.getByTestId("shortcut-list")).toContainText("Help");

  await page.getByTestId("help-nav-whats-new").click();
  const entries = parseChangelog(readFileSync("CHANGELOG.md", "utf8")).flatMap((s) => s.entries);
  await expect(page.getByTestId("help-whats-new-entry")).toHaveCount(entries.length);
});

test("the sign-in app forms link to the steps in Help", async ({ inbox: page }) => {
  await openSettings(page, "apps");
  await page.getByTestId("client-id-help").click();
  await expect(page.getByTestId("help")).toHaveAttribute("data-page", "your-own-app");
  await expect(page.getByTestId("help-content")).toContainText("Entra admin center");
});
