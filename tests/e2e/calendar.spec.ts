import type { Page } from "@playwright/test";
import { localAt } from "../harness/calendar-fixtures.ts";
import { test, expect, showCalendarPane, snap, syncNow, openSettings } from "./fixtures";

const event = (page: Page, remoteId: string) => page.locator(`[data-testid=cal-event][data-remote-id="${remoteId}"]`);

const showCalendar = showCalendarPane;

test("the right pane shows today's calendar by default, synced from Graph; 0 toggles Recent Opens", async ({ inbox: page }, testInfo) => {
  // Calendar is the first tab and the default.
  await expect(page.getByTestId("day-view")).toBeVisible();
  await expect(page.getByTestId("pane-tab-calendar")).toHaveAttribute("aria-selected", "true");
  const tabs = await page.locator("[data-testid^=pane-tab-]").evaluateAll((els) => els.map((e) => e.getAttribute("data-testid")));
  expect(tabs).toEqual(["pane-tab-calendar", "pane-tab-recent"]);
  await expect(page.getByTestId("day-label")).toContainText("Today");
  await expect(event(page, "evt-invite")).toContainText("Design review");
  await expect(event(page, "evt-lunch")).toContainText("Lunch with Sam");
  await expect(page.getByTestId("now-line")).toBeVisible();
  await snap(page, testInfo, "day-view");

  // 0 switches to Recent Opens, and again back to the calendar.
  await page.keyboard.press("0");
  await expect(page.getByTestId("day-view")).toHaveCount(0);
  await expect(page.getByTestId("pane-tab-recent")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("0");
  await expect(page.getByTestId("day-view")).toBeVisible();
});

test("overlapping events sit side by side", async ({ inbox: page, fake }) => {
  // The "happening now" fixture moves with the clock and can overlap anything; leave it out.
  await fake.control("event-remove", { id: "evt-helpdesk" });
  await syncNow(page, fake);
  await expect(event(page, "evt-helpdesk")).toHaveCount(0);
  await showCalendar(page);
  await expect(event(page, "evt-1on1")).toHaveAttribute("data-cols", "2");
  await expect(event(page, "evt-focus")).toHaveAttribute("data-cols", "2");
  await expect(event(page, "evt-focus")).toHaveAttribute("data-col", "1");
  await expect(event(page, "evt-lunch")).toHaveAttribute("data-cols", "1");
});

test("day navigation: tomorrow, all-day events, and back to today", async ({ inbox: page }) => {
  await showCalendar(page);
  await page.getByTestId("day-next").click();
  await expect(page.getByTestId("day-label")).toContainText("Tomorrow");
  await expect(event(page, "evt-dentist")).toBeVisible();
  await expect(page.getByTestId("now-line")).toHaveCount(0);
  await page.getByTestId("day-next").click();
  await expect(page.locator('[data-testid=all-day-event][data-remote-id="evt-offsite"]')).toContainText("Team offsite");
  await page.getByTestId("day-today").click();
  await expect(page.getByTestId("day-label")).toContainText("Today");
});

test("new, changed and removed events arrive through delta sync", async ({ inbox: page, fake }) => {
  await showCalendar(page);
  await fake.control("event", { id: "evt-new", subject: "Coffee with Hannah", start: localAt(0, 17).toISOString(), end: localAt(0, 17, 30).toISOString() });
  await fake.control("event-remove", { id: "evt-lunch" });
  await syncNow(page, fake);
  await expect(event(page, "evt-new")).toContainText("Coffee with Hannah");
  await expect(event(page, "evt-lunch")).toHaveCount(0);
});

test("an expired calendar delta link triggers a full resync", async ({ inbox: page, fake }) => {
  await showCalendar(page);
  await fake.control("config", { expireCalendarDelta: true });
  await fake.control("event", { id: "evt-late", subject: "Late addition", start: localAt(0, 18).toISOString(), end: localAt(0, 19).toISOString() });
  await syncNow(page, fake);
  await expect(event(page, "evt-late")).toBeVisible();
  expect(fake.state.requests.some((r) => r.path.endsWith("/calendarView/delta") && r.status === 410)).toBe(true);
  await expect(event(page, "evt-invite")).toBeVisible(); // nothing lost in the resync
});

test("the calendar tab is remembered", async ({ inbox: page }) => {
  await showCalendar(page);
  await page.reload();
  await expect(page.getByTestId("thread-row").first()).toBeVisible();
  await expect(page.getByTestId("day-view")).toBeVisible();
});

test("g c opens the week view; arrows change week, t returns, Esc goes back", async ({ inbox: page }, testInfo) => {
  await page.keyboard.press("g");
  await page.keyboard.press("c");
  const week = page.getByTestId("week-view");
  await expect(week).toBeVisible();
  await expect(page.locator("[data-testid=week-day][data-today=true]")).toHaveCount(1);
  await expect(page.locator('[data-testid=week-event][data-remote-id="evt-invite"]')).toBeVisible();
  await expect(page.getByTestId("week-now-line")).toBeVisible();
  await snap(page, testInfo, "week-view");

  // Mail keys are off here: j doesn't move a hidden selection, e doesn't archive.
  await page.keyboard.press("e");
  await page.keyboard.press("ArrowRight");
  await expect(page.locator("[data-testid=week-day][data-today=true]")).toHaveCount(0);
  await page.keyboard.press("t");
  await expect(page.locator("[data-testid=week-day][data-today=true]")).toHaveCount(1);

  await page.keyboard.press("Escape");
  await expect(week).toHaveCount(0);
  await expect(page.locator('[data-testid=thread-row][data-conversation-id="conv-hostile"]')).toHaveCount(1);
});

test("all-day events span their days in the week view", async ({ inbox: page }) => {
  await page.getByTestId("rail-calendar").click();
  const offsite = page.locator('[data-testid=week-all-day][data-remote-id="evt-offsite"]');
  // The offsite is two days from now, so it may fall in next week.
  if (!(await offsite.count())) await page.getByTestId("week-next").click();
  await expect(offsite).toContainText("Team offsite");
});

test("clicking an event shows its details, from the week view and the day view", async ({ inbox: page }, testInfo) => {
  await page.getByTestId("rail-calendar").click();
  await page.locator('[data-testid=week-event][data-remote-id="evt-invite"]').click();
  const details = page.getByTestId("event-details");
  await expect(page.getByTestId("event-title")).toHaveText("Design review");
  await expect(page.getByTestId("event-location")).toHaveText("Room 4 / Teams");
  await expect(page.getByTestId("event-attendees")).toContainText("Sam Carter");
  await expect(page.getByTestId("event-response")).toContainText("Not answered yet");
  await expect(page.getByTestId("event-join")).toBeVisible();
  await snap(page, testInfo, "event-details");
  await page.keyboard.press("Escape");
  await expect(details).toHaveCount(0);

  await page.keyboard.press("Escape"); // back to the inbox
  await showCalendar(page);
  await event(page, "evt-lunch").click();
  await expect(page.getByTestId("event-title")).toHaveText("Lunch with Sam");
  await page.getByTestId("event-close").click();
  await expect(details).toHaveCount(0);
});

test("recurring meetings and bare events show their details (subject, organiser, location)", async ({ inbox: page, fake }) => {
  await showCalendar(page);
  // Sync fetched only the master delta doesn't list (opening a popup fetches more, on purpose).
  expect(fake.state.requests.filter((r) => r.path === "/v1.0/me/events/evt-sync-series")).toHaveLength(1);
  expect(fake.state.requests.filter((r) => r.path === "/v1.0/me/events/evt-standup-series")).toHaveLength(0);
  // Standup occurrences arrive bare; details come from the series master in the delta.
  const standup = page.locator("[data-testid=cal-event]", { hasText: "Standup" });
  await page.getByTestId("day-grid").evaluate((el) => (el.scrollTop = 8 * 44));
  await expect(standup).toHaveCount(1);
  await standup.click();
  await expect(page.getByTestId("event-title")).toHaveText("Standup");
  await expect(page.getByTestId("event-organizer")).toContainText("Sam Carter");
  await page.keyboard.press("Escape");

  // Team sync's master isn't in the delta at all; the app fetched it by id.
  await page.keyboard.press("g");
  await page.keyboard.press("c");
  await expect(page.locator("[data-testid=week-event]", { hasText: "Team sync" })).toHaveCount(1);
  // A bare one-off event was fetched by id too.
  const planning = page.locator('[data-testid=week-event][data-remote-id="evt-bare"]');
  if (!(await planning.count())) await page.getByTestId("week-next").click(); // tomorrow may be next week
  await expect(planning).toContainText("Quarterly planning");
  await planning.click();
  await expect(page.getByTestId("event-location")).toHaveText("Board room");
});

test("the left sidebar switches between the inbox and Calendar, and so do 1 and 2", async ({ inbox: page }, testInfo) => {
  const railMail = page.getByTestId("rail-mailbox-inbox");
  const railCal = page.getByTestId("rail-calendar");
  await expect(railMail).toHaveAttribute("data-active", "true");
  await snap(page, testInfo, "rail");

  await page.keyboard.press("2");
  await expect(page.getByTestId("week-view")).toBeVisible();
  await expect(railCal).toHaveAttribute("data-active", "true");
  await page.keyboard.press("1");
  await expect(page.getByTestId("thread-list")).toBeVisible();
  await expect(railMail).toHaveAttribute("data-active", "true");

  await railCal.click();
  await expect(page.getByTestId("week-view")).toBeVisible();
  await railMail.click();
  await expect(page.getByTestId("thread-list")).toBeVisible();

  // Typing a 2 in the composer stays text.
  await page.keyboard.press("c");
  await page.getByTestId("compose-body").pressSequentially("2 tickets");
  await expect(page.getByTestId("compose-body")).toHaveAttribute("data-text", "2 tickets");
  await expect(page.getByTestId("week-view")).toHaveCount(0);
});

test("the first day of the week is a setting the week view follows", async ({ inbox: page }, testInfo) => {
  const firstDay = () => page.getByTestId("week-day").first().textContent();
  await page.keyboard.press("2");
  expect(await firstDay()).toMatch(/^Mon/);

  await openSettings(page, "calendar");
  await page.getByTestId("week-start-select").selectOption("0");
  await snap(page, testInfo, "week-start-setting");
  await page.keyboard.press("Escape");
  await expect.poll(firstDay).toMatch(/^Sun/);
  await expect(page.locator("[data-testid=week-day][data-today=true]")).toHaveCount(1); // still this week

  await page.reload();
  await expect(page.getByTestId("thread-row").first()).toBeVisible();
  await page.keyboard.press("2");
  await expect.poll(firstDay).toMatch(/^Sun/);
});

test("event details with a long guest list scroll instead of overflowing the window", async ({ inbox: page, fake }) => {
  const attendees = Array.from({ length: 60 }, (_, i) => [`Guest ${i + 1}`, `guest${i + 1}@example.com`]);
  await fake.control("event", { id: "evt-big", subject: "Campaign afternoon tea", start: localAt(0, 20).toISOString(), end: localAt(0, 21).toISOString(), attendees });
  await syncNow(page, fake);
  await page.keyboard.press("2");
  await page.locator('[data-testid=week-event][data-remote-id="evt-big"]').click();

  await expect(page.getByTestId("event-guest-count")).toContainText("60 guests");
  await page.getByTestId("event-attendees-expand").click(); // the whole list, not the collapsed first 5
  const viewport = page.viewportSize() ?? (await page.evaluate(() => ({ width: innerWidth, height: innerHeight })));
  const panel = (await page.getByTestId("event-panel").boundingBox())!;
  expect(panel.y + panel.height).toBeLessThanOrEqual(viewport.height);
  await expect(page.getByTestId("event-close")).toBeInViewport();
  await expect(page.getByTestId("event-actions")).toBeInViewport();
  const scroll = page.getByTestId("event-scroll");
  expect(await scroll.evaluate((el) => el.scrollHeight > el.clientHeight)).toBe(true);
});

test("events you created look confirmed, not pending", async ({ inbox: page }) => {
  await showCalendar(page);
  await page.getByTestId("day-grid").evaluate((el) => (el.scrollTop = 12 * 44));
  await expect(event(page, "evt-own")).toHaveAttribute("data-tone", "confirmed"); // response "none", you organised it
  await expect(event(page, "evt-lunch")).toHaveAttribute("data-tone", "confirmed");
  await expect(event(page, "evt-invite")).toHaveAttribute("data-tone", "pending"); // someone else's, unanswered
  await event(page, "evt-own").click();
  await expect(page.getByTestId("event-response")).toHaveCount(0);
});

test("week view day headers line up with the day columns below them", async ({ inbox: page }) => {
  await page.keyboard.press("2");
  const headers = await page.getByTestId("week-day").evaluateAll((els) => els.map((e) => e.getBoundingClientRect()).map((r) => [r.left, r.right]));
  const columns = await page.getByTestId("week-column").evaluateAll((els) => els.map((e) => e.getBoundingClientRect()).map((r) => [r.left, r.right]));
  expect(columns).toHaveLength(7);
  for (let i = 0; i < 7; i++) {
    expect(Math.abs(headers[i][0] - columns[i][0])).toBeLessThanOrEqual(1);
    expect(Math.abs(headers[i][1] - columns[i][1])).toBeLessThanOrEqual(1);
  }
});

test("after midnight, a day view left on today moves to the new today", async ({ inbox: page }) => {
  await showCalendarPane(page);
  const label = (d: Date) => d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
  const now = new Date();
  const nextMorning = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1, 0, 5);
  await expect(page.getByTestId("day-label")).toContainText(label(now));

  // The PC's clock passes midnight; the app notices on its next check (here: the window coming back).
  await page.clock.setSystemTime(nextMorning);
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await expect(page.getByTestId("day-label")).toContainText(label(nextMorning));
  await expect(page.getByTestId("day-label")).toContainText("Today");
});

test("after midnight, the week view highlights the new today (and moves on a week if it starts one)", async ({ inbox: page }) => {
  await page.keyboard.press("g");
  await page.keyboard.press("c");
  await expect(page.getByTestId("week-view")).toBeVisible();
  const todayCol = page.locator("[data-testid=week-day][data-today=true]");
  const now = new Date();
  await expect(todayCol).toContainText(String(now.getDate()));

  const nextMorning = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1, 0, 5);
  await page.clock.setSystemTime(nextMorning);
  await page.evaluate(() => document.dispatchEvent(new Event("visibilitychange")));
  await expect(todayCol).toContainText(String(nextMorning.getDate()));
  await expect(page.getByTestId("week-today")).toBeDisabled();
});
