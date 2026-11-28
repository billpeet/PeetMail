import type { Page } from "@playwright/test";
import { parseQuickEvent } from "../../src/lib/quickAdd.ts";
import { test, expect, snap, syncNow, showCalendarPane } from "./fixtures";

type Fake = Parameters<Parameters<typeof test>[2]>[0]["fake"];
const creates = (fake: Fake) => fake.state.sent.filter((s) => s.kind === "create-event");

async function quickAdd(page: Page, text: string) {
  await page.keyboard.press("Shift+C");
  const input = page.getByTestId("quick-add-input");
  await expect(input).toBeFocused();
  await input.fill(text);
}

test("Shift+C, type it in plain words, check the preview, Enter creates it", async ({ inbox: page, fake }, testInfo) => {
  const text = "Lunch with Sam fri 1pm @ Café Beetroot";
  const expected = parseQuickEvent(text);
  if (!expected.ok) throw new Error("fixture text should parse");
  await quickAdd(page, text);
  await expect(page.getByTestId("quick-add-title")).toHaveText("Lunch with Sam");
  await expect(page.getByTestId("quick-add-when")).toContainText(expected.event.start.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }));
  await expect(page.getByTestId("quick-add-when")).toContainText("Café Beetroot");
  await snap(page, testInfo, "quick-add");
  await page.keyboard.press("Enter");

  await expect(page.getByTestId("quick-add")).toHaveCount(0);
  await expect(page.getByTestId("toast")).toContainText("Created");
  // Shows straight away in the sidebar, on its day (the fixture's own "Lunch with Sam" is today).
  await showCalendarPane(page);
  await expect(page.locator("[data-testid=cal-event][data-remote-id^='local:'], [data-testid=cal-event][data-remote-id^='evt-created-']")).toBeVisible();

  // ...and the server gets the right instant (UTC) and location.
  await expect.poll(() => creates(fake).length, { timeout: 10_000 }).toBe(1);
  const body = creates(fake)[0].body;
  expect(body.subject).toBe("Lunch with Sam");
  expect(body.start).toEqual({ dateTime: expected.event.start.toISOString().replace(/\.\d{3}Z$/, ""), timeZone: "UTC" });
  expect(body.location).toEqual({ displayName: "Café Beetroot" });
  // The placeholder is swapped for the server's event.
  await expect(page.locator(`[data-testid=cal-event][data-remote-id="${creates(fake)[0].id}"]`)).toBeVisible();
});

test("all-day events go in as dates in your time zone", async ({ inbox: page, fake }) => {
  await quickAdd(page, "Offsite planning fri all day");
  await expect(page.getByTestId("quick-add-when")).toContainText("all day");
  await page.keyboard.press("Enter");
  await expect.poll(() => creates(fake).length, { timeout: 10_000 }).toBe(1);
  const body = creates(fake)[0].body;
  expect(body.isAllDay).toBe(true);
  expect(body.start.dateTime).toMatch(/^\d{4}-\d{2}-\d{2}T00:00:00$/);
  expect(body.start.timeZone).toBe(Intl.DateTimeFormat().resolvedOptions().timeZone);
  await page.keyboard.press("2");
  await expect(page.getByTestId("week-view")).toBeVisible();
  // On a weekend "fri" is next week's (weeks start on Monday): go to the week it's in.
  const monday = new Date();
  monday.setHours(0, 0, 0, 0);
  monday.setDate(monday.getDate() - ((monday.getDay() + 6) % 7));
  const days = Math.round((new Date(`${body.start.dateTime.slice(0, 10)}T00:00:00`).getTime() - monday.getTime()) / 86_400_000);
  for (let w = 0; w < Math.floor(days / 7); w++) await page.keyboard.press("ArrowRight");
  await expect(page.locator("[data-testid=week-all-day]", { hasText: "Offsite planning" })).toBeVisible();
});

test("text it can't place gets a hint, and Enter does nothing", async ({ inbox: page, fake }) => {
  await quickAdd(page, "just some words");
  await expect(page.getByTestId("quick-add-hint")).toContainText("time or a day");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("quick-add")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("quick-add")).toHaveCount(0);
  expect(creates(fake)).toEqual([]);
});

test("if the server rejects it, the placeholder goes away and you're told", async ({ inbox: page, fake }) => {
  await fake.control("config", { failWrites: 400 });
  await quickAdd(page, "Doomed meeting tomorrow 3pm");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("quick-add")).toHaveCount(0); // global keys are off until it closes
  await expect(page.getByTestId("toast")).toContainText("Couldn't sync a change", { timeout: 10_000 });
  await showCalendarPane(page);
  await expect(page.getByTestId("day-label")).toContainText("Tomorrow"); // the view went to the new event's day
  await expect(page.locator("[data-testid=cal-event]", { hasText: "Doomed meeting" })).toHaveCount(0);
});

test("a placeholder survives a full calendar resync while the server is unreachable", async ({ inbox: page, fake }) => {
  await fake.control("config", { failWrites: 500 }); // transient, but not a throttle (503 is retried in-call for ~30s)
  await quickAdd(page, "Patient meeting tomorrow 4pm");
  await page.keyboard.press("Enter");
  // Global keys (Ctrl+R below) are off while the dialog is open, and it closes only once
  // the create call returns; under load a key pressed sooner is dropped.
  await expect(page.getByTestId("quick-add")).toHaveCount(0);
  await fake.control("config", { expireCalendarDelta: true });
  await syncNow(page, fake);
  await showCalendarPane(page);
  await expect(page.getByTestId("day-label")).toContainText("Tomorrow");
  const ev = page.locator("[data-testid=cal-event]", { hasText: "Patient meeting" });
  await expect(ev).toHaveAttribute("data-remote-id", /^local:/);

  await fake.control("config", { failWrites: 0 });
  await page.waitForTimeout(1200); // first retry backoff
  await page.keyboard.press("Control+r");
  await expect(ev).toHaveAttribute("data-remote-id", /^evt-created-/, { timeout: 10_000 });
});
