<script lang="ts">
  // Day calendar for the right pane: all-day chips, a 24h grid with overlap columns,
  // and a now-line. Scrolls to the working part of the day.
  import { onMount, tick } from "svelte";
  import { ChevronLeft, ChevronRight, Plus } from "@lucide/svelte";
  import { mail } from "#lib/mail.svelte.js";
  import JoinChip from "./JoinChip.svelte";
  import { calendar, eventTimes } from "#lib/calendar.svelte.js";
  import { layoutDay } from "#lib/eventLayout.js";
  import { eventTone, TONE_CLASSES } from "#lib/eventTone.js";
  import type { CalendarEvent } from "#lib/api.js";

  let { onselect }: { onselect?: (e: CalendarEvent) => void } = $props();

  const HOUR = 44; // px per hour
  let scroller: HTMLDivElement | undefined = $state();
  let now = $state(new Date());

  const allDay = $derived(calendar.dayEvents.filter((e) => e.isAllDay));
  const placed = $derived(
    layoutDay(
      calendar.dayEvents.filter((e) => !e.isAllDay).map((e) => ({ ...eventTimes(e), event: e })),
      calendar.day,
    ),
  );
  const nowTop = $derived(((now.getTime() - calendar.day.getTime()) / 3_600_000) * HOUR);

  const dayLabel = $derived.by(() => {
    const diff = Math.round((calendar.day.getTime() - calendar.todayStart.getTime()) / 86_400_000);
    const date = calendar.day.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
    return diff === 0 ? `Today · ${date}` : diff === 1 ? `Tomorrow · ${date}` : diff === -1 ? `Yesterday · ${date}` : date;
  });

  const time = (d: Date) => d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const hourLabel = (h: number) => new Date(2000, 0, 1, h).toLocaleTimeString(undefined, { hour: "numeric" });

  /** Colour by how you've answered: solid when going, outlined when undecided, faded when declined. */
  const tone = (e: CalendarEvent) => TONE_CLASSES[eventTone(e)];

  async function scrollToWorkday() {
    await tick();
    if (!scroller) return;
    // An invite in context: bring its event into view, an hour of lead-in above it.
    const focus = calendar.focus && !calendar.focus.isAllDay ? eventTimes(calendar.focus).start : null;
    const hour = focus
      ? Math.max(0, (focus.getTime() - calendar.day.getTime()) / 3_600_000 - 1)
      : calendar.isToday
        ? Math.max(0, Math.min(now.getHours() - 1, 16))
        : 8;
    scroller.scrollTop = hour * HOUR;
  }

  $effect(() => {
    calendar.day; // re-scroll when the day changes
    calendar.focus;
    scrollToWorkday();
  });

  // A recurring invite focuses the series master: its occurrence is the one to highlight.
  const focused = (e: CalendarEvent) => !!calendar.focus && (calendar.focus.remoteId === e.remoteId || calendar.focus.remoteId === e.seriesMasterId);

  // A new day: the now-line moves at once, not on the next minute's tick.
  $effect(() => {
    calendar.todayStart;
    now = new Date();
  });

  onMount(() => {
    calendar.load();
    // The minute tick checks the date too, so "now" and "today" never disagree past midnight.
    const timer = setInterval(() => {
      now = new Date();
      calendar.rollover(now);
    }, 60_000);
    return () => clearInterval(timer);
  });
</script>

<div data-testid="day-view" class="flex min-h-0 flex-1 flex-col">
  <div class="flex items-center gap-1 px-4 pb-2">
    <span data-testid="day-label" class="flex-1 truncate text-sm font-semibold text-ink-100">{dayLabel}</span>
    <button data-testid="day-new" title="New event (Shift+C)" aria-label="New event" onclick={() => (mail.overlay = "quickadd")} class="icon-btn size-7"><Plus size={16} /></button>
    <button data-testid="day-prev" title="Previous day" aria-label="Previous day" onclick={() => calendar.shiftDay(-1)} class="icon-btn size-7"><ChevronLeft size={16} /></button>
    <button data-testid="day-today" onclick={() => calendar.today()} disabled={calendar.isToday} class="rounded-md px-2 py-1 text-xs text-ink-300 hover:bg-ink-800 hover:text-ink-100 disabled:opacity-40">Today</button>
    <button data-testid="day-next" title="Next day" aria-label="Next day" onclick={() => calendar.shiftDay(1)} class="icon-btn size-7"><ChevronRight size={16} /></button>
  </div>

  {#if allDay.length}
    <div class="space-y-1 border-b border-ink-800 px-4 pb-2">
      {#each allDay as e (e.id)}
        <button data-testid="all-day-event" data-remote-id={e.remoteId} data-tone={eventTone(e)} onclick={() => onselect?.(e)} class="block w-full truncate rounded px-2 py-0.5 text-left text-xs {tone(e)}">{#if mail.accounts.length > 1}<span data-testid="event-account" title={mail.accountOf(e.accountId)?.email} class={["mr-1 inline-block size-2 shrink-0 rounded-full align-middle", mail.accountColor(e.accountId)]}></span>{/if}{e.subject}</button>
      {/each}
    </div>
  {/if}

  <div bind:this={scroller} data-testid="day-grid" class="relative min-h-0 flex-1 overflow-y-auto">
    <div class="relative" style:height="{24 * HOUR}px">
      {#each Array.from({ length: 24 }, (_, h) => h) as h (h)}
        <div class="absolute inset-x-0 flex" style:top="{h * HOUR}px">
          <span class="w-14 shrink-0 -translate-y-2 pr-2 text-right text-[11px] text-ink-500">{h === 0 ? "" : hourLabel(h)}</span>
          <span class="flex-1 border-t border-ink-800"></span>
        </div>
      {/each}

      <div class="absolute inset-y-0 right-3 left-14">
        {#each placed as p (p.item.event.id)}
          {@const e = p.item.event}
          {@const px = (p.height / 60) * HOUR - 2}
          <button
            data-testid="cal-event"
            data-remote-id={e.remoteId}
            data-col={p.col}
            data-cols={p.cols}
            data-response={e.response ?? "none"}
            data-tone={eventTone(e)}
            data-focused={focused(e)}
            onclick={() => onselect?.(e)}
            title="{e.subject}{e.location ? ` · ${e.location}` : ''}"
            class={[
              "absolute overflow-hidden rounded-md px-2 text-left leading-tight",
              px < 22 ? "flex items-center text-[10px]" : "py-1 text-xs",
              tone(e),
              // The invite you're reading, in context with the rest of the day.
              focused(e) && "z-20 ring-2 ring-amber-300 shadow-lg",
            ]}
            style:top="{(p.top / 60) * HOUR + 1}px"
            style:height="{px}px"
            style:left="calc({(p.col / p.cols) * 100}% + 1px)"
            style:width="calc({100 / p.cols}% - 2px)"
          >
            <JoinChip {e} {now} testid="cal-join" compact={px < 16} />
            <span class="block truncate font-semibold">{#if mail.accounts.length > 1}<span data-testid="event-account" title={mail.accountOf(e.accountId)?.email} class={["mr-1 inline-block size-2 shrink-0 rounded-full align-middle", mail.accountColor(e.accountId)]}></span>{/if}{e.subject || "(no title)"}</span>
            <!-- Extra lines only when there's room for the whole line. -->
            {#if px >= 36}
              <span class="block truncate text-ink-300">{time(p.item.start)} – {time(p.item.end)}</span>
            {/if}
            {#if px >= 54 && e.location}<span class="block truncate text-ink-500">{e.location}</span>{/if}
          </button>
        {/each}

        {#if calendar.isToday}
          <div data-testid="now-line" class="pointer-events-none absolute inset-x-0 z-10 border-t-2 border-red-400" style:top="{nowTop}px">
            <span class="absolute -top-[5px] -left-1 size-2 rounded-full bg-red-400"></span>
          </div>
        {/if}
      </div>
    </div>
  </div>
</div>
