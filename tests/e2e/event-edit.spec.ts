import type { Page } from "@playwright/test";
import { test, expect, showCalendarPane, snap } from "./fixtures";

type Fake = { state: { sent: Record<string, any>[] } };
const event = (page: Page, remoteId: string) => page.locator(`[data-testid=cal-event][data-remote-id="${remoteId}"]`);
const sentOf = (fake: Fake, kind: string) => fake.state.sent.filter((s) => s.kind === kind);

async function openEvent(page: Page, remoteId: string) {
  await showCalendarPane(page);
  await event(page, remoteId).scrollIntoViewIfNeeded();
  await event(page, remoteId).click();
  await expect(page.getByTestId("event-details")).toBeVisible();
}

async function openEditor(page: Page, remoteId: string, scope?: "o" | "s") {
  await openEvent(page, remoteId);
  await page.keyboard.press("e");
  if (scope) {
    await expect(page.getByTestId("event-scope")).toBeVisible();
    await page.keyboard.press(scope);
  }
  await expect(page.getByTestId("event-editor")).toBeVisible();
  await expect(page.getByTestId("editor-loading")).toHaveCount(0);
}

test("editing your own event: title, reminder, a guest and a rich description; only the changes are sent", async ({ inbox: page, fake }, testInfo) => {
  await openEditor(page, "evt-lunch");
  await expect(page.getByTestId("editor-mode")).toHaveText("Edit event");
  await expect(page.getByTestId("editor-title")).toHaveValue("Lunch with Sam");
  await expect(page.getByTestId("editor-location")).toHaveValue("Café Beetroot");
  await expect(page.getByTestId("editor-reminder")).toHaveValue("none");
  await expect(page.getByTestId("editor-private")).toBeChecked();

  await page.getByTestId("editor-title").fill("Lunch with Sam & Priya");
  await page.getByTestId("editor-reminder").selectOption("10");
  const guest = page.getByTestId("editor-attendee-input");
  await guest.pressSequentially("priya");
  await expect(page.getByTestId("editor-attendee-suggestions")).toContainText("priya@fabrikam.io");
  await guest.press("Enter");
  await expect(page.getByTestId("editor-attendee")).toHaveText(/Priya Nair/);
  const rich = page.getByTestId("rich-editor");
  await rich.click();
  await page.keyboard.type("Bring the ");
  await page.keyboard.press("Control+b");
  await page.keyboard.type("receipts");
  await snap(page, testInfo, "event-editor");
  await page.keyboard.press("Control+Enter");

  await expect(page.getByTestId("event-editor")).toHaveCount(0);
  await expect(page.getByTestId("toast")).toContainText("Saved; guests will get an update");
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(1);
  const u = sentOf(fake, "update-event")[0];
  expect(u.id).toBe("evt-lunch");
  expect(u.body.subject).toBe("Lunch with Sam & Priya");
  expect(u.body.reminderMinutesBeforeStart).toBe(10);
  expect(u.body.isReminderOn).toBe(true);
  expect(u.body.attendees).toEqual([{ emailAddress: { address: "priya@fabrikam.io", name: "Priya Nair" }, type: "required" }]);
  expect(u.body.body.content).toContain("<strong>receipts</strong>");
  // Unchanged fields stay out.
  expect(Object.keys(u.body).sort()).toEqual(["attendees", "body", "isReminderOn", "reminderMinutesBeforeStart", "subject"]);
  await expect(event(page, "evt-lunch")).toContainText("Lunch with Sam & Priya");
});

test("moving the start keeps the length; the new times are sent in UTC", async ({ inbox: page, fake }) => {
  await openEditor(page, "evt-lunch"); // 13:00–14:00
  await page.getByTestId("editor-start-time").fill("15:30");
  await page.getByTestId("editor-start-time").dispatchEvent("change");
  await expect(page.getByTestId("editor-end-time")).toHaveValue("16:30");
  await page.getByTestId("editor-save").click();
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(1);
  const b = sentOf(fake, "update-event")[0].body;
  expect(b.start.timeZone).toBe("UTC");
  const start = new Date(`${b.start.dateTime}Z`);
  const end = new Date(`${b.end.dateTime}Z`);
  expect([start.getHours(), start.getMinutes()]).toEqual([15, 30]);
  expect(end.getTime() - start.getTime()).toBe(60 * 60_000);
  expect(Object.keys(b).sort()).toEqual(["end", "isAllDay", "start"]);
});

test("a recurring event asks what to edit, and the editor says which", async ({ inbox: page, fake }, testInfo) => {
  // Just this occurrence.
  await openEditor(page, "evt-review-1", "o");
  await expect(page.getByTestId("editor-scope")).toHaveAttribute("data-series", "false");
  await expect(page.getByTestId("editor-scope")).toContainText("Editing this occurrence only");
  await page.getByTestId("editor-title").fill("Weekly review (moved agenda)");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(1);
  expect(sentOf(fake, "update-event")[0]).toMatchObject({ id: "evt-review-1", body: { subject: "Weekly review (moved agenda)" } });

  // The whole series: the master, with its pattern shown.
  await openEditor(page, "evt-review-1", "s");
  const scope = page.getByTestId("editor-scope");
  await expect(scope).toHaveAttribute("data-series", "true");
  await expect(scope).toContainText("Editing the whole series");
  await expect(scope).toContainText(/Every week on \w+day/);
  await expect(page.getByTestId("editor-mode")).toHaveText("Edit series");
  await expect(page.getByTestId("editor-title")).toHaveValue("Weekly review"); // the series' own title
  await snap(page, testInfo, "editor-series");
  await page.getByTestId("editor-location").fill("Room 9");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(2);
  expect(sentOf(fake, "update-event")[1]).toMatchObject({ id: "evt-review-series", body: { location: { displayName: "Room 9" } } });
});

test("guests can only change their own show-as and reminder", async ({ inbox: page, fake }) => {
  await openEditor(page, "evt-invite");
  await expect(page.getByTestId("editor-guest")).toBeVisible();
  await expect(page.getByTestId("editor-title")).toBeDisabled();
  await expect(page.getByTestId("editor-location")).toBeDisabled();
  await expect(page.getByTestId("editor-attendee-input")).toHaveCount(0);
  await page.getByTestId("editor-show-as").selectOption("free");
  await page.keyboard.press("Control+Enter");
  await expect(page.getByTestId("toast")).toHaveText("Saved");
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(1);
  expect(sentOf(fake, "update-event")[0].body).toEqual({ showAs: "free" });
});

test("deleting: your own event, cancelling a meeting (one occurrence and the series), and a guest removing", async ({ inbox: page, fake }, testInfo) => {
  // Your own event, no guests: a plain delete.
  await openEvent(page, "evt-own");
  await page.keyboard.press("#");
  await expect(page.getByTestId("event-delete-confirm")).toContainText("Delete this event?");
  await page.keyboard.press("Control+Enter");
  await expect(page.getByTestId("toast")).toContainText("Deleted the event");
  await expect(event(page, "evt-own")).toHaveCount(0);
  await expect.poll(() => sentOf(fake, "delete-event").map((s) => s.id), { timeout: 10_000 }).toEqual(["evt-own"]);

  // A meeting you organise: cancelling tells the guests, with a message.
  await openEvent(page, "evt-review-1");
  await page.getByTestId("event-delete").click();
  await page.getByTestId("scope-one").click();
  await expect(page.getByTestId("event-delete-confirm")).toContainText("Cancel this occurrence of the meeting? Guests get a cancellation.");
  await page.getByTestId("event-delete-message").fill("Out sick, sorry");
  await snap(page, testInfo, "cancel-meeting");
  await page.getByTestId("event-delete-go").click();
  await expect(page.getByTestId("toast")).toContainText("Cancelled the meeting occurrence");
  await expect.poll(() => sentOf(fake, "cancel-event"), { timeout: 10_000 }).toEqual([{ kind: "cancel-event", id: "evt-review-1", comment: "Out sick, sorry" }]);

  // The whole series goes, occurrences and all (today's is gone: next week's is left).
  await page.keyboard.press("2");
  await page.getByTestId("week-next").click();
  const weekReview = page.locator("[data-testid=week-event]", { hasText: "Weekly review" });
  await expect(weekReview.first()).toBeVisible();
  await weekReview.first().click();
  await page.keyboard.press("Delete");
  await page.keyboard.press("s");
  await expect(page.getByTestId("event-delete-confirm")).toContainText("Cancel every meeting in this series?");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "cancel-event").map((s) => s.id), { timeout: 10_000 }).toEqual(["evt-review-1", "evt-review-series"]);
  await expect(weekReview).toHaveCount(0);

  // A meeting you were invited to: removed from your calendar, organiser not told.
  await page.keyboard.press("Escape");
  await openEvent(page, "evt-invite");
  await page.keyboard.press("#");
  await expect(page.getByTestId("event-delete-confirm")).toContainText("Remove this from your calendar?");
  await page.getByTestId("event-delete-go").click();
  await expect.poll(() => sentOf(fake, "delete-event").map((s) => s.id), { timeout: 10_000 }).toEqual(["evt-own", "evt-invite"]);
});

test("quick add's Tab opens the full editor; a new meeting with guests, a description and privacy", async ({ inbox: page, fake }, testInfo) => {
  await page.keyboard.press("Shift+C");
  await page.getByTestId("quick-add-input").fill("Design sync tomorrow 2pm");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("event-editor")).toBeVisible();
  await expect(page.getByTestId("editor-mode")).toHaveText("New event");
  await expect(page.getByTestId("editor-title")).toHaveValue("Design sync");
  await expect(page.getByTestId("editor-start-time")).toHaveValue("14:00");
  await expect(page.getByTestId("editor-end-time")).toHaveValue("14:30");
  await page.getByTestId("editor-attendee-input").fill("sam@contoso.com");
  await page.getByTestId("editor-attendee-input").press("Enter");
  await page.getByTestId("editor-private").check();
  await page.getByTestId("rich-editor").click();
  await page.keyboard.type("Agenda: mockups");
  await expect(page.getByTestId("editor-save-label")).toHaveText("Send");
  await expect(page.getByTestId("editor-save")).toContainText("CtrlEnter");
  await snap(page, testInfo, "editor-new");
  await page.keyboard.press("Control+Enter");
  await expect(page.getByTestId("toast")).toContainText("Sent “Design sync” to 1 guest");
  await expect.poll(() => sentOf(fake, "create-event").length, { timeout: 10_000 }).toBe(1);
  const b = sentOf(fake, "create-event")[0].body;
  expect(b.subject).toBe("Design sync");
  expect(b.attendees).toEqual([{ emailAddress: { address: "sam@contoso.com", name: "sam@contoso.com" }, type: "required" }]);
  expect(b.sensitivity).toBe("private");
  expect(b.body.content).toContain("Agenda: mockups");
});

test("Esc with unsaved changes asks first; nothing is sent", async ({ inbox: page, fake }) => {
  await openEditor(page, "evt-lunch");
  await page.getByTestId("editor-title").fill("Changed my mind");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("editor-discard")).toBeVisible();
  await expect(page.getByTestId("event-editor")).toBeVisible();
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("event-editor")).toHaveCount(0);
  await page.waitForTimeout(500);
  expect(sentOf(fake, "update-event")).toEqual([]);

  // No changes: Esc just closes.
  await openEditor(page, "evt-lunch");
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("event-editor")).toHaveCount(0);
});

test("a Microsoft 365 location suggests rooms; picking one books it as a room guest", async ({ inbox: page, fake }, testInfo) => {
  await page.keyboard.press("Shift+C");
  await page.getByTestId("quick-add-input").fill("Planning tomorrow 10am");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("event-editor")).toBeVisible();
  const loc = page.getByTestId("editor-location");
  await loc.click();
  // All the rooms on focus, narrowed as you type.
  await expect(page.getByTestId("editor-room")).toHaveCount(3);
  await loc.fill("hud");
  await expect(page.getByTestId("editor-room")).toHaveText(["Huddle 1"]);
  await loc.fill("");
  await loc.pressSequentially("board");
  await snap(page, testInfo, "editor-rooms");
  await loc.press("Enter");
  await expect(loc).toHaveValue("Boardroom");
  const room = page.getByTestId("editor-location-room");
  await expect(room).toHaveAttribute("data-address", "boardroom@contoso.com");
  // Booked by inviting it, but it shows by the location, not as a guest.
  await expect(page.getByTestId("editor-attendee")).toHaveCount(0);

  // Choosing another room swaps it, rather than booking both.
  await loc.fill("");
  await loc.pressSequentially("Room 4");
  await page.getByTestId("editor-room").first().click();
  await expect(room).toHaveCount(1);
  await expect(room).toHaveAttribute("data-address", "room4@contoso.com");

  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "create-event").length, { timeout: 10_000 }).toBe(1);
  const b = sentOf(fake, "create-event")[0].body;
  expect(b.location).toEqual({ displayName: "Room 4", locationEmailAddress: "room4@contoso.com", locationType: "conferenceRoom" });
  expect(b.attendees).toEqual([{ emailAddress: { address: "room4@contoso.com", name: "Room 4" }, type: "resource" }]);
});

test("typing over a booked room, or its ×, un-books it", async ({ inbox: page, fake }) => {
  await openEditor(page, "evt-lunch");
  const loc = page.getByTestId("editor-location");
  await loc.fill("");
  await loc.pressSequentially("Huddle");
  await loc.press("Enter");
  const room = page.getByTestId("editor-location-room");
  await expect(room).toHaveCount(1);
  // Un-booking the room clears the location it filled in.
  await page.getByTestId("editor-location-room-remove").click();
  await expect(room).toHaveCount(0);
  await expect(loc).toHaveValue("");
  // Pick it again, then type something else: no room any more. Typing straight after the ×
  // click, then pausing, used to let the × click's blur close the list under you.
  await loc.pressSequentially("Huddle");
  await page.waitForTimeout(300);
  await loc.press("Enter");
  await expect(room).toHaveCount(1);
  await loc.press("End");
  await loc.pressSequentially(" (or the kitchen)");
  await expect(room).toHaveCount(0);
  await expect(page.getByTestId("editor-location-room")).toHaveCount(0);
  await page.keyboard.press("Escape"); // close the suggestions
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(1);
  const b = sentOf(fake, "update-event")[0].body;
  expect(b.location).toEqual({ displayName: "Huddle 1 (or the kitchen)" });
  expect(b.attendees).toBeUndefined();
});

test("an event with a booked room reopens showing it as a room", async ({ inbox: page }) => {
  await openEditor(page, "evt-lunch");
  const loc = page.getByTestId("editor-location");
  await loc.fill("");
  await loc.pressSequentially("Boardroom");
  await loc.press("Enter");
  await page.keyboard.press("Control+Enter");
  await expect(page.getByTestId("event-editor")).toHaveCount(0);
  await expect(page.getByTestId("toast")).toContainText("Saved");
  await page.waitForTimeout(1500); // let the outbox send it
  await openEditor(page, "evt-lunch");
  await expect(page.getByTestId("editor-location")).toHaveValue("Boardroom");
  await expect(page.getByTestId("editor-location-room")).toBeVisible();
  await expect(page.getByTestId("editor-location-room")).toHaveAttribute("data-address", "boardroom@contoso.com");
});

test("a tenant that refuses the room list: allowing it reconnects the account in place and the rooms appear", async ({ inbox: page, fake }, testInfo) => {
  fake.state.config.roomsNeedScope = "User.ReadBasic.All";
  await openEditor(page, "evt-lunch");
  await page.getByTestId("editor-location").click();
  const prompt = page.getByTestId("editor-rooms-permission");
  await expect(prompt).toContainText("permission to read basic directory info");
  await snap(page, testInfo, "editor-rooms-permission");
  const signIns = fake.state.authorizations.length;
  await page.getByTestId("editor-rooms-grant").click();
  // A sign-in for the same account, asking for the defaults plus the directory permission.
  await expect(page.getByTestId("editor-room").first()).toBeVisible({ timeout: 15_000 });
  await expect(page.getByTestId("editor-room")).toHaveCount(3);
  const auth = fake.state.authorizations[signIns];
  expect(auth.login_hint).toBe("boris@peetmail.test");
  expect(auth.scope.split(" ")).toEqual(expect.arrayContaining(["Mail.ReadWrite", "Calendars.ReadWrite", "User.ReadBasic.All"]));
  await expect(prompt).toHaveCount(0);

  // The editor is still there with the event; the account still syncs after a refresh
  // (which may only ask for scopes it was granted).
  await expect(page.getByTestId("editor-title")).toHaveValue("Lunch with Sam");
  await page.keyboard.press("Escape");
  await page.keyboard.press("Escape");
  for (const k of fake.state.accessTokens.keys()) fake.state.accessTokens.set(k, 0);
  const before = fake.state.requests.length;
  await page.keyboard.press("Control+r");
  await expect.poll(() => fake.state.requests.slice(before).some((r: any) => r.path === "/oauth/token" && r.status === 200), { timeout: 15_000 }).toBe(true);
  await expect.poll(() => fake.state.requests.slice(before).some((r: any) => r.status === 200 && String(r.path).includes("/delta")), { timeout: 15_000 }).toBe(true);
  await expect(page.getByTestId("sync-status")).not.toHaveAttribute("data-state", "error");
});

test("when even that isn't enough, it asks for admin approval; a refusal says so and changes nothing", async ({ inbox: page, fake }) => {
  fake.state.config.roomsNeedScope = "Something.Else";
  fake.state.config.denyScopes = ["Place.Read.All"];
  await openEditor(page, "evt-lunch");
  await page.getByTestId("editor-location").click();
  await page.getByTestId("editor-rooms-grant").click();
  // Directory permission granted, still refused: next is the admin-approved Places API.
  await expect(page.getByTestId("editor-rooms-grant")).toHaveText("Request admin approval", { timeout: 15_000 });
  await page.getByTestId("editor-rooms-grant").click();
  await expect(page.getByTestId("editor-rooms-error")).toHaveText("Your organisation needs an admin to approve this.", { timeout: 15_000 });
  await expect(page.getByTestId("editor-room")).toHaveCount(0);

  // With approval granted, the Places API lists them.
  fake.state.config.denyScopes = [];
  await page.getByTestId("editor-location").click();
  await page.getByTestId("editor-rooms-grant").click();
  await expect(page.getByTestId("editor-room")).toHaveCount(3, { timeout: 15_000 });
});

test("Not now hides the room prompt for that account", async ({ inbox: page, fake }) => {
  fake.state.config.roomsNeedScope = "User.ReadBasic.All";
  await openEditor(page, "evt-lunch");
  await page.getByTestId("editor-location").click();
  await page.getByTestId("editor-rooms-not-now").click();
  await expect(page.getByTestId("editor-rooms-permission")).toHaveCount(0);
  await page.getByTestId("editor-title").click();
  await page.getByTestId("editor-location").click();
  await page.waitForTimeout(300);
  await expect(page.getByTestId("editor-rooms-permission")).toHaveCount(0);
});

test("focusing a location that's already set lists every room, and Tab moves on without replacing it", async ({ inbox: page }) => {
  await openEditor(page, "evt-lunch");
  const loc = page.getByTestId("editor-location");
  await loc.click();
  await expect(page.getByTestId("editor-room")).toHaveCount(3);
  await loc.press("Tab");
  await expect(loc).toHaveValue("Café Beetroot");
  await expect(page.getByTestId("editor-location-room")).toHaveCount(0);
  // Arrowing in, then Enter, picks.
  await loc.click();
  await loc.press("ArrowDown");
  await loc.press("Enter");
  await expect(loc).toHaveValue("Boardroom");
});

test("Tab flow: quick add hands over at the location, then location → guests with nothing in between", async ({ inbox: page, fake }) => {
  await page.keyboard.press("Shift+C");
  await page.getByTestId("quick-add-input").fill("Planning tomorrow 10am");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("event-editor")).toBeVisible();
  // Title and time came from the text: the cursor starts at the location.
  const loc = page.getByTestId("editor-location");
  await expect(loc).toBeFocused();
  // Empty location, Tab: straight on to guests, no room picked.
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("editor-attendee-input")).toBeFocused();
  await expect(page.getByTestId("editor-location-room")).toHaveCount(0);
  await expect(loc).toHaveValue("");
  // Pick a room, add a guest, Tab from the location lands in guests again (no room chip to tab past).
  await page.keyboard.press("Shift+Tab");
  await expect(loc).toBeFocused();
  await page.keyboard.type("Huddle");
  await page.keyboard.press("Enter");
  await expect(loc).toHaveValue("Huddle 1");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("editor-attendee-input")).toBeFocused();
  await page.keyboard.type("sam@contoso.com");
  await page.keyboard.press("Enter");
  await expect(page.getByTestId("editor-attendee")).toHaveCount(1);
  // Guests → the description's text (not its formatting buttons) → show as.
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("rich-editor")).toBeFocused();
  await page.keyboard.type("Agenda");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("editor-show-as")).toBeFocused();
  await page.keyboard.press("Shift+Tab");
  await page.keyboard.press("Shift+Tab");
  await expect(page.getByTestId("editor-attendee-input")).toBeFocused();
  // Backspace removes the person, never the room.
  await page.keyboard.press("Backspace");
  await page.keyboard.press("Backspace");
  await expect(page.getByTestId("editor-attendee")).toHaveCount(0);
  await expect(page.getByTestId("editor-location-room")).toHaveAttribute("data-address", "huddle1@contoso.com");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "create-event").length, { timeout: 10_000 }).toBe(1);
  expect(sentOf(fake, "create-event")[0].body.attendees).toEqual([{ emailAddress: { address: "huddle1@contoso.com", name: "Huddle 1" }, type: "resource" }]);
  expect(sentOf(fake, "create-event")[0].body.body.content).toContain("Agenda");
});

test("quick add without a time understood: Tab starts the editor at the title", async ({ inbox: page }) => {
  await page.keyboard.press("Shift+C");
  await page.getByTestId("quick-add-input").fill("Planning");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("editor-title")).toBeFocused();
  await expect(page.getByTestId("editor-title")).toHaveValue("Planning");
});

// ---------- repeat ----------

const ymd = (d: Date) => `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
const WEEKDAY = ["sunday", "monday", "tuesday", "wednesday", "thursday", "friday", "saturday"];

test("a new series: a preset, then custom (every 2 weeks on two days, 6 times), sent in local time", async ({ inbox: page, fake }, testInfo) => {
  const tomorrow = new Date();
  tomorrow.setDate(tomorrow.getDate() + 1);
  const dayAfter = new Date(tomorrow);
  dayAfter.setDate(dayAfter.getDate() + 1);
  await page.keyboard.press("Shift+C");
  await page.getByTestId("quick-add-input").fill("Team sync tomorrow 9am");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("event-editor")).toBeVisible();
  const select = page.getByTestId("editor-repeat-select");
  await expect(select).toHaveValue("none");
  await select.selectOption("weekly");
  await expect(page.getByTestId("repeat-summary")).toHaveText(`Every week on ${WEEKDAY[tomorrow.getDay()].replace(/^./, (c) => c.toUpperCase())}`);

  await select.selectOption("custom");
  await page.getByTestId("repeat-interval").fill("2");
  const second = WEEKDAY[dayAfter.getDay()].slice(0, 3);
  await page.locator(`[data-testid=repeat-day][data-day=${second}]`).click();
  await page.getByTestId("repeat-end").selectOption("count");
  await page.getByTestId("repeat-count").fill("6");
  await expect(page.getByTestId("repeat-summary")).toContainText("Every 2 weeks on");
  await expect(page.getByTestId("repeat-summary")).toContainText("6 times");
  await snap(page, testInfo, "editor-repeat-custom");
  await page.keyboard.press("Control+Enter");

  await expect.poll(() => sentOf(fake, "create-event").length, { timeout: 10_000 }).toBe(1);
  const b = sentOf(fake, "create-event")[0].body;
  expect(b.start).toEqual({ dateTime: `${ymd(tomorrow)}T09:00:00`, timeZone: zone });
  expect(b.end).toEqual({ dateTime: `${ymd(tomorrow)}T09:30:00`, timeZone: zone });
  expect(b.recurrence.pattern).toMatchObject({ type: "weekly", interval: 2, firstDayOfWeek: "monday" });
  expect([...b.recurrence.pattern.daysOfWeek].sort()).toEqual([WEEKDAY[tomorrow.getDay()], WEEKDAY[dayAfter.getDay()]].sort());
  expect(b.recurrence.range).toEqual({ type: "numbered", startDate: ymd(tomorrow), numberOfOccurrences: 6, recurrenceTimeZone: zone });
});

test("a preset follows the start date; a pattern that can't be saved says why", async ({ inbox: page, fake }) => {
  await page.keyboard.press("Shift+C");
  await page.getByTestId("quick-add-input").fill("Gym tomorrow 7am");
  await page.keyboard.press("Tab");
  await page.getByTestId("editor-repeat-select").selectOption("weekly");
  const start = page.getByTestId("editor-start-date");
  const d = new Date();
  d.setDate(d.getDate() + 2);
  await start.fill(ymd(d));
  await start.dispatchEvent("change");
  await expect(page.getByTestId("repeat-summary")).toHaveText(`Every week on ${WEEKDAY[d.getDay()].replace(/^./, (c) => c.toUpperCase())}`);

  // Custom weekly with no days: not saved.
  await page.getByTestId("editor-repeat-select").selectOption("custom");
  await page.locator(`[data-testid=repeat-day][aria-pressed=true]`).click();
  await expect(page.getByTestId("repeat-problem")).toHaveText("Pick at least one day to repeat on");
  await page.keyboard.press("Control+Enter");
  await expect(page.getByTestId("editor-error")).toHaveText("Pick at least one day to repeat on");
  await page.waitForTimeout(300);
  expect(sentOf(fake, "create-event")).toEqual([]);
});

test("editing a series' pattern: the end date; one occurrence can't change it", async ({ inbox: page, fake }) => {
  // One occurrence: the pattern is shown, not editable.
  await openEditor(page, "evt-review-1", "o");
  await expect(page.getByTestId("editor-repeat-select")).toHaveCount(0);
  await expect(page.getByTestId("editor-repeat")).toHaveText("Part of a series");
  await page.keyboard.press("Escape");

  await openEditor(page, "evt-review-1", "s");
  await expect(page.getByTestId("editor-repeat-select")).toHaveValue("weekly"); // recognised as the preset
  await page.getByTestId("repeat-end").selectOption("until");
  const until = new Date();
  until.setDate(until.getDate() + 30);
  await page.getByTestId("repeat-until").fill(ymd(until));
  await page.getByTestId("repeat-until").dispatchEvent("change");
  await expect(page.getByTestId("repeat-change-note")).toBeVisible();
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(1);
  const { id, body } = sentOf(fake, "update-event")[0];
  expect(id).toBe("evt-review-series");
  expect(body.recurrence.range).toMatchObject({ type: "endDate", endDate: ymd(until) });
  expect(body.start.timeZone).toBe(zone); // with its local time
  expect(body.start.dateTime).toMatch(/T10:00:00$/);
});

test("stopping a series warns first, then sends no pattern", async ({ inbox: page, fake }) => {
  await openEditor(page, "evt-review-1", "s");
  await page.getByTestId("editor-repeat-select").selectOption("none");
  await expect(page.getByTestId("repeat-stop-warning")).toContainText("the other occurrences are removed and guests are told");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(1);
  expect(sentOf(fake, "update-event")[0].body).toEqual({ recurrence: null });
});

test("a single event can be made to repeat", async ({ inbox: page, fake }) => {
  await openEditor(page, "evt-lunch");
  await page.getByTestId("editor-repeat-select").selectOption("weekdays");
  await expect(page.getByTestId("repeat-summary")).toHaveText("Every weekday");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(1);
  const b = sentOf(fake, "update-event")[0].body;
  expect(b.recurrence.pattern.daysOfWeek).toEqual(["monday", "tuesday", "wednesday", "thursday", "friday"]);
  expect(b.start).toEqual({ dateTime: expect.stringMatching(/T13:00:00$/), timeZone: zone });
  // Reopened, it's the series it now is.
  await expect(page.getByTestId("event-editor")).toHaveCount(0);
  await page.waitForTimeout(1500);
  await openEditor(page, "evt-lunch");
  await expect(page.getByTestId("editor-repeat-select")).toHaveValue("weekdays");
});

test("a new meeting gets a Teams link: Ctrl+M ticks it, and the event comes back with a join link", async ({ inbox: page, fake }, testInfo) => {
  await page.keyboard.press("Shift+C");
  await page.getByTestId("quick-add-input").fill("Estimating catch up tomorrow 10am");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("event-editor")).toBeVisible();
  const online = page.getByTestId("editor-online-meeting");
  await expect(online).not.toBeChecked();
  await expect(online.locator("xpath=..")).toContainText("Teams meeting");
  await page.keyboard.press("Control+m");
  await expect(online).toBeChecked();
  await expect(page.getByTestId("editor-online-note")).toBeVisible();
  await snap(page, testInfo, "editor-teams");
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "create-event").length, { timeout: 10_000 }).toBe(1);
  const b = sentOf(fake, "create-event")[0].body;
  expect([b.isOnlineMeeting, b.onlineMeetingProvider]).toEqual([true, "teamsForBusiness"]);

  // Synced back, it has Teams' join link.
  const id = sentOf(fake, "create-event")[0].id;
  await page.keyboard.press("Control+r");
  await expect(event(page, id)).toBeVisible({ timeout: 10_000 });
  await event(page, id).click();
  await expect(page.getByTestId("event-join-link")).toContainText("Teams");
});

test("an existing event can get a Teams link, and then shows it has one", async ({ inbox: page, fake }) => {
  await openEditor(page, "evt-lunch");
  await page.getByTestId("editor-online-meeting").check();
  await page.keyboard.press("Control+Enter");
  await expect.poll(() => sentOf(fake, "update-event").length, { timeout: 10_000 }).toBe(1);
  expect(sentOf(fake, "update-event")[0].body).toEqual({ isOnlineMeeting: true, onlineMeetingProvider: "teamsForBusiness" });

  await expect(page.getByTestId("event-editor")).toHaveCount(0);
  await openEditor(page, "evt-lunch");
  const online = page.getByTestId("editor-online-meeting");
  await expect(online).toBeChecked();
  await expect(online).toBeDisabled();
  await expect(online.locator("xpath=..")).toContainText("Has an online meeting");
});

test("a calendar that offers no online meetings shows no toggle; one that offers Skype says so", async ({ inbox: page, fake }) => {
  await fake.control("config", { meetingProviders: { allowedOnlineMeetingProviders: [], defaultOnlineMeetingProvider: "unknown" } });
  await page.keyboard.press("Shift+C");
  await page.getByTestId("quick-add-input").fill("Coffee tomorrow 3pm");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("event-editor")).toBeVisible();
  await page.waitForTimeout(500);
  await expect(page.getByTestId("editor-online-meeting")).toHaveCount(0);
  await page.getByTestId("editor-cancel").click();
  await page.getByTestId("editor-cancel").click();
  await expect(page.getByTestId("event-editor")).toHaveCount(0);

  await fake.control("config", { meetingProviders: { allowedOnlineMeetingProviders: ["skypeForConsumer"], defaultOnlineMeetingProvider: "skypeForConsumer" } });
  await page.keyboard.press("Shift+C");
  await page.getByTestId("quick-add-input").fill("Coffee tomorrow 3pm");
  await page.keyboard.press("Tab");
  await expect(page.getByTestId("editor-online-meeting").locator("xpath=..")).toContainText("Skype meeting");
});
