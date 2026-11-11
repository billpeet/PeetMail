import { beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("./api", () => ({ api: { listEvents: () => Promise.resolve([]) } }));
vi.mock("./accounts.svelte", () => ({ visibility: { isHidden: () => false } }));

const { calendar, startOfDay, startOfWeek, addDays } = await import("./calendar.svelte");

const today = startOfDay(new Date());
const tomorrow = addDays(today, 1);
/** Just after midnight, the next day. */
const nextMorning = new Date(tomorrow.getTime() + 5 * 60_000);
// Each test starts on today; rolling back keeps the store's own "today" in step.
beforeEach(() => {
  calendar.rollover(new Date());
  calendar.focus = null;
  calendar.day = today;
  calendar.weekStart = startOfWeek(today, calendar.weekStartsOn);
});

describe("a new day", () => {
  it("moves the day view from what was today to the new today", () => {
    calendar.rollover(nextMorning);
    expect(calendar.day).toEqual(tomorrow);
  });

  it("leaves a day view you'd moved to another date where it is", () => {
    const friday = addDays(today, 4);
    calendar.day = friday;
    calendar.rollover(nextMorning);
    expect(calendar.day).toEqual(friday);
  });

  it("does nothing until the date actually changes", () => {
    calendar.rollover(new Date(today.getTime() + 23 * 3_600_000));
    expect(calendar.day).toEqual(today);
  });

  it("moves the week view on when the new day starts a new week", () => {
    const lastDayOfWeek = addDays(startOfWeek(today, calendar.weekStartsOn), 6);
    // Pretend it's the week's last day, then roll into the next week.
    calendar.rollover(new Date(lastDayOfWeek.getTime() + 12 * 3_600_000));
    calendar.weekStart = startOfWeek(lastDayOfWeek, calendar.weekStartsOn);
    calendar.rollover(new Date(addDays(lastDayOfWeek, 1).getTime() + 60_000));
    expect(calendar.weekStart).toEqual(startOfWeek(addDays(lastDayOfWeek, 1), calendar.weekStartsOn));
  });

  it("after the PC's time zone changes, the week on screen is this week in the new zone", () => {
    // Put back by name: unsetting TZ doesn't make Node look the zone up again.
    const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    try {
      process.env.TZ = "Pacific/Auckland";
      calendar.rollover(new Date());
      calendar.weekStart = startOfWeek(calendar.todayStart, calendar.weekStartsOn);
      process.env.TZ = "America/Los_Angeles";
      calendar.rollover(new Date());
      expect(calendar.isThisWeek).toBe(true);
      expect(calendar.weekStart).toEqual(startOfWeek(new Date(), calendar.weekStartsOn));
    } finally {
      process.env.TZ = zone;
    }
  });

  it("with an invite open, the day view stays on the meeting; closing it goes to the new today", async () => {
    const meetingDay = addDays(today, 3);
    await calendar.showInContext({ remoteId: "m", response: null, isAllDay: false, startAt: new Date(meetingDay.getTime() + 10 * 3_600_000).toISOString(), endAt: new Date(meetingDay.getTime() + 11 * 3_600_000).toISOString() } as never);
    calendar.rollover(nextMorning);
    expect(calendar.day).toEqual(meetingDay);
    await calendar.clearContext();
    expect(calendar.day).toEqual(tomorrow);
  });
});
