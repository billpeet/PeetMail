// Calendar state for the day view (right pane) and, later, the week view.
import { visibility } from "./accounts.svelte";
import { api, type CalendarEvent } from "./api";

const DAY = 86_400_000;

/** Local midnight of the day containing `d`. */
export function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

export function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

/** YYYY-MM-DD of a local date. */
export function localDate(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

export function isAllDay(e: CalendarEvent): boolean {
  return e.isAllDay;
}

/** Start/end as Dates. All-day dates become local midnights. */
export function eventTimes(e: CalendarEvent): { start: Date; end: Date } {
  if (e.isAllDay) {
    const [ys, ms, ds] = e.startAt.split("-").map(Number);
    const [ye, me, de] = e.endAt.split("-").map(Number);
    return { start: new Date(ys, ms - 1, ds), end: new Date(ye, me - 1, de) };
  }
  return { start: new Date(e.startAt), end: new Date(e.endAt) };
}

/** Events overlapping local days `[from, to)`. */
export async function fetchEvents(from: Date, to: Date): Promise<CalendarEvent[]> {
  // Accounts toggled off in the footer legend stay out of the calendar.
  const events = await api.listEvents(from.toISOString(), to.toISOString(), localDate(from), localDate(to));
  return events.filter((e) => !visibility.isHidden(e.accountId));
}

/** First day of the week containing `d` (local midnight). `weekStartsOn`: 0 = Sunday ... 6 = Saturday. */
export function startOfWeek(d: Date, weekStartsOn = 1): Date {
  const day = startOfDay(d);
  const offset = (day.getDay() - weekStartsOn + 7) % 7;
  return addDays(day, -offset);
}

/** Open the editor: a new event (optionally prefilled from quick add) or an existing one. */
export type EditRequest =
  | {
      mode: "create";
      accountId: number | null;
      prefill?: { subject: string; start: Date; end: Date; allDay: boolean; location: string | null };
      /** Where the cursor starts: past what quick add already filled in. */
      focus?: "title" | "location";
    }
  | { mode: "edit"; event: CalendarEvent; series: boolean };

class CalendarStore {
  /** Day shown in the day view (local midnight). */
  day = $state(startOfDay(new Date()));
  dayEvents = $state.raw<CalendarEvent[]>([]);
  /** First day of the week (0 = Sunday ... 6 = Saturday); from preferences. */
  weekStartsOn = $state(1);
  /** First day of the week shown in the week view. */
  weekStart = $state(startOfWeek(new Date()));
  weekEvents = $state.raw<CalendarEvent[]>([]);
  /** Event whose details are open. */
  selected = $state.raw<CalendarEvent | null>(null);
  /** What the event editor is doing (overlay "editevent"). */
  editing = $state.raw<EditRequest | null>(null);
  /**
   * While an invite is open: the event to show in context. The day view jumps to its day
   * and highlights it; the right pane shows the calendar. Cleared when the invite closes.
   */
  focus = $state.raw<CalendarEvent | null>(null);
  #dayBeforeFocus: Date | null = null;
  #seq = 0;
  #weekSeq = 0;
  /** Today (local midnight) as of the last check: the views label and highlight by it. */
  todayStart = $state(startOfDay(new Date()));

  /**
   * After midnight (or waking the PC on a later day), the day and week views still on what
   * was today move to the new today. Views you'd moved to another date stay put.
   */
  rollover(now = new Date()) {
    const today = startOfDay(now);
    const was = this.todayStart;
    if (today.getTime() === was.getTime()) return;
    this.todayStart = today;
    // An open invite holds the day view; closing it goes back to the new today instead.
    if (this.focus) {
      if (this.#dayBeforeFocus?.getTime() === was.getTime()) this.#dayBeforeFocus = today;
    } else if (this.day.getTime() === was.getTime()) {
      this.goToDay(today);
    }
    // Compared as instants, not recomputed: after a time zone change, "was" and the week on
    // screen are both midnights in the old zone, and the week is rebuilt in the new one.
    const showedToday = this.weekStart.getTime() <= was.getTime() && was.getTime() < addDays(this.weekStart, 7).getTime();
    if (showedToday && this.weekStart.getTime() !== startOfWeek(today, this.weekStartsOn).getTime()) this.showWeekOf(today);
  }

  /** Checks for a new day every half minute and whenever the window comes back. Returns a stop function. */
  watchDate(): () => void {
    const check = () => this.rollover();
    const timer = setInterval(check, 30_000);
    document.addEventListener("visibilitychange", check);
    window.addEventListener("focus", check);
    return () => {
      clearInterval(timer);
      document.removeEventListener("visibilitychange", check);
      window.removeEventListener("focus", check);
    };
  }

  get isThisWeek(): boolean {
    return this.weekStart.getTime() === startOfWeek(this.todayStart, this.weekStartsOn).getTime();
  }

  /** Applies the week-start preference, keeping the same week on screen. */
  setWeekStartsOn(day: number) {
    if (day === this.weekStartsOn) return;
    const anchor = this.isThisWeek ? this.todayStart : this.weekStart;
    this.weekStartsOn = day;
    this.weekStart = startOfWeek(anchor, day);
    return this.loadWeek();
  }

  /** Shows the week containing `d`. */
  showWeekOf(d: Date) {
    this.weekStart = startOfWeek(d, this.weekStartsOn);
    return this.loadWeek();
  }

  async loadWeek() {
    const seq = ++this.#weekSeq;
    const events = await fetchEvents(this.weekStart, addDays(this.weekStart, 7));
    if (seq === this.#weekSeq) this.weekEvents = events;
  }

  shiftWeek(n: number) {
    this.weekStart = addDays(this.weekStart, 7 * n);
    return this.loadWeek();
  }

  thisWeek() {
    this.rollover();
    return this.showWeekOf(this.todayStart);
  }

  /** Reloads everything that's showing (after a sync or a change). */
  async refresh() {
    await Promise.all([this.load(), this.loadWeek()]);
    if (this.selected) {
      const id = this.selected.id;
      const fresh = [...this.dayEvents, ...this.weekEvents].find((e) => e.id === id);
      if (fresh) this.selected = fresh;
    }
  }

  /** Shows an invite's event in the day view (its day, highlighted). */
  async showInContext(e: CalendarEvent) {
    if (this.focus?.remoteId === e.remoteId && this.focus.response === e.response) return;
    if (!this.focus) this.#dayBeforeFocus = this.day;
    this.focus = e;
    await this.goToDay(eventTimes(e).start);
  }

  /** Back to where the day view was before the invite opened. */
  async clearContext() {
    if (!this.focus) return;
    this.focus = null;
    const back = this.#dayBeforeFocus ?? new Date();
    this.#dayBeforeFocus = null;
    await this.goToDay(back);
  }

  get isToday(): boolean {
    return this.day.getTime() === this.todayStart.getTime();
  }

  async load() {
    const seq = ++this.#seq;
    const events = await fetchEvents(this.day, addDays(this.day, 1));
    if (seq === this.#seq) this.dayEvents = events;
  }

  async goToDay(d: Date) {
    this.day = startOfDay(d);
    await this.load();
  }

  shiftDay(n: number) {
    return this.goToDay(addDays(this.day, n));
  }

  today() {
    this.rollover();
    return this.goToDay(this.todayStart);
  }
}

export const calendar = new CalendarStore();
export { DAY };
