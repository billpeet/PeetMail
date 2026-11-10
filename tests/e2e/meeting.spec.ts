import { signIn } from "../harness/app.ts";
import { localAt } from "../harness/calendar-fixtures.ts";
import { test, expect, row, snap, syncNow, waitForSyncIdle, showRecentPane } from "./fixtures";

type Fake = Parameters<Parameters<typeof test>[2]>[0]["fake"];
type Page = import("@playwright/test").Page;

const labels = (page: Page) => page.getByTestId("meeting-label").allTextContents();

async function openMovedMeeting(page: Page) {
  await row(page, "conv-update").click();
  await expect(page.getByTestId("invite-card")).toBeVisible();
}

test("a meeting moved after you accepted shows your acceptance and asks about the new time", async ({ inbox: page, fake }, testInfo) => {
  await openMovedMeeting(page);
  await expect.poll(() => labels(page)).toEqual(["Invitation", "You accepted", "Updated invitation"]);

  const card = page.getByTestId("invite-card");
  await expect(card).toContainText("Updated");
  // The old time is shown struck through.
  const oldStart = localAt(2, 14, 0);
  await expect(page.getByTestId("invite-changed")).toContainText("Time changed");
  await expect(page.getByTestId("invite-changed")).toContainText(oldStart.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }));
  await expect(page.getByTestId("invite-location-changed")).toContainText("Room 1");
  await expect(page.getByTestId("invite-status")).toHaveText("You accepted the earlier time. This update needs your response.");
  await snap(page, testInfo, "moved-meeting");

  await page.keyboard.press("y");
  await expect(page.getByTestId("invite-status")).toHaveText("You're going");
  await expect.poll(async () => (await fake.control("event?id=evt-update")).responseStatus.response, { timeout: 10_000 }).toBe("accepted");
});

async function signInWith(page: Page, fake: Fake, config: Record<string, unknown>) {
  await fake.control("config", config);
  await signIn(page, fake.clientId);
  await waitForSyncIdle(page);
}

test("invites are recognised even when sync responses leave out @odata.type", async ({ page, fake }) => {
  await signInWith(page, fake, { omitODataType: true });
  await openMovedMeeting(page);
  await expect.poll(() => labels(page)).toEqual(["Invitation", "You accepted", "Updated invitation"]);
});

test("if Graph rejects the meeting-type field, sync falls back and @odata.type still identifies invites", async ({ page, fake }) => {
  await signInWith(page, fake, { rejectMeetingSelect: true });
  const inbox = fake.state.requests.filter((r) => r.path.endsWith("/AAMkAGI2-inbox/messages/delta"));
  expect(inbox[0].status).toBe(400);
  expect(inbox.find((r) => r.status === 200)!.query).not.toContain("meetingMessageType");
  await row(page, "conv-update").click();
  await expect(page.getByTestId("invite-card")).toBeVisible();
});

test("viewing an invite jumps the calendar sidebar to its day and highlights it among that day's events", async ({ inbox: page }, testInfo) => {
  await showRecentPane(page); // the sidebar on Recent Opens: viewing the invite switches it, leaving restores it
  await openMovedMeeting(page);

  // The sidebar switches to the calendar on the meeting's day.
  await expect(page.getByTestId("day-view")).toBeVisible();
  const day = localAt(3, 10).toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
  await expect(page.getByTestId("day-label")).toContainText(day);
  const meeting = page.locator('[data-testid=cal-event][data-remote-id="evt-update"]');
  await expect(meeting).toHaveAttribute("data-focused", "true");
  await expect(meeting).toBeInViewport();
  // In context: the clashing call sits beside it.
  const clash = page.locator('[data-testid=cal-event][data-remote-id="evt-clash"]');
  await expect(clash).toBeVisible();
  await expect(meeting).toHaveAttribute("data-cols", "2");
  await snap(page, testInfo, "invite-in-context");

  // Answering updates the highlighted event too.
  await page.keyboard.press("y");
  await expect(meeting).toHaveAttribute("data-response", "accepted");

  // Leaving the invite restores the sidebar.
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("pane-tab-recent")).toHaveAttribute("aria-selected", "true");
  await page.keyboard.press("0");
  await expect(page.getByTestId("day-label")).toContainText("Today");
});

test("an invite to a recurring meeting gets an RSVP card that answers the whole series", async ({ inbox: page, fake }, testInfo) => {
  // Reported: a weekly meeting's invite showed no card, because its event is the series master.
  const start = localAt(6, 10, 30);
  const end = localAt(6, 11, 0);
  const ymd = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  const weekday = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"][start.getDay()];
  const until = localAt(6 + 7 * 25, 10, 30);
  await fake.control("event", {
    id: "evt-weekly", subject: "Estimations catch up", start: start.toISOString(), end: end.toISOString(),
    location: "Upstairs boardroom", response: "notResponded", organizer: ["Sam Carter", "sam.carter@contoso.com"],
    recurrence: {
      pattern: { type: "weekly", interval: 1, daysOfWeek: [weekday], firstDayOfWeek: "monday" },
      range: { type: "endDate", startDate: ymd(start), endDate: ymd(until) },
    },
  });
  await fake.control("event", { id: "evt-weekly-1", seriesMasterId: "evt-weekly", start: start.toISOString(), end: end.toISOString() });
  const { ids } = await fake.control("deliver", { subject: "Estimations catch up", eventId: "evt-weekly" });
  await syncNow(page, fake);
  await row(page, `conv-${ids[0]}`).click();

  const card = page.getByTestId("invite-card");
  await expect(card).toBeVisible();
  await expect(card).toContainText("Estimations catch up");
  await expect(card).toContainText(start.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" }));
  await expect(page.getByTestId("invite-recurrence")).toContainText(`Every week on ${weekday[0].toUpperCase()}${weekday.slice(1)}, until`);
  await expect(page.getByTestId("invite-status")).toHaveText("You haven't responded yet");
  // The sidebar shows the first occurrence, highlighted.
  const occurrence = page.locator('[data-testid=cal-event][data-remote-id="evt-weekly-1"]');
  await expect(occurrence).toHaveAttribute("data-focused", "true");
  await snap(page, testInfo, "recurring-invite");

  await page.keyboard.press("y");
  await expect(page.getByTestId("invite-status")).toHaveText("You're going");
  await expect.poll(async () => (await fake.control("event?id=evt-weekly")).responseStatus.response, { timeout: 10_000 }).toBe("accepted");
});

test("a failed invite lookup says so once, and the card appears when it next succeeds", async ({ inbox: page, fake }) => {
  await fake.control("config", { failInvite: true });
  await row(page, "conv-invite").click();
  const toast = page.getByTestId("toast");
  await expect(toast).toContainText("Couldn't load the meeting details");
  await expect(page.getByTestId("invite-card")).toHaveCount(0);
  await expect(toast).toBeHidden({ timeout: 10_000 });

  // New mail refreshes the list and the open conversation, retrying the lookup, without toasting again.
  const lookups = () => fake.state.requests.filter((r) => r.path.endsWith("/messages/msg-invite") && r.query.includes("eventMessage")).length;
  const before = lookups();
  await fake.control("deliver", { subject: "Something else" });
  await syncNow(page, fake);
  await expect.poll(lookups).toBeGreaterThan(before);
  // Its failure lands just after the request. A one-off check: toBeHidden would wait out a
  // repeat toast's own timeout and pass anyway.
  await page.waitForTimeout(2000);
  expect(await toast.count()).toBe(0);

  await fake.control("config", { failInvite: false });
  await fake.control("deliver", { subject: "And another" });
  await syncNow(page, fake);
  await expect(page.getByTestId("invite-card")).toBeVisible();
});
