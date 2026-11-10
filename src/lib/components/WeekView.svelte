<script lang="ts">
  // Full calendar (g c): a week (starting on the day set in Settings) with an all-day row, overlap columns per day,
  // and a now-line in today's column. ←/→ change week, t jumps to today, Esc goes back.
  import { onMount, tick } from "svelte";
  import { ArrowLeft, ChevronLeft, ChevronRight, Plus } from "@lucide/svelte";
  import { addDays, calendar, eventTimes } from "#lib/calendar.svelte.js";
  import { layoutDay } from "#lib/eventLayout.js";
  import { eventTone, TONE_CLASSES } from "#lib/eventTone.js";
  import { mail } from "#lib/mail.svelte.js";
  import JoinChip from "./JoinChip.svelte";
  import type { CalendarEvent } from "#lib/api.js";

  const HOUR = 48;
  let scroller: HTMLDivElement | undefined = $state();
  let now = $state(new Date());

  const days = $derived(Array.from({ length: 7 }, (_, i) => addDays(calendar.weekStart, i)));
  const todayMs = $derived(calendar.todayStart.getTime());
  // A new day: today's column and the now-line move at once, not on the next minute's tick.
  $effect(() => {
    calendar.todayStart;
    now = new Date();
  });

  const timedByDay = $derived(
    days.map((d) =>
      layoutDay(
        calendar.weekEvents.filter((e) => !e.isAllDay).map((e) => ({ ...eventTimes(e), event: e })),
        d,
      ),
    ),
  );

  /** All-day events with the day columns they cover (clipped to the week). */
  const allDay = $derived(
    calendar.weekEvents
      .filter((e) => e.isAllDay)
      .map((e) => {
        const { start, end } = eventTimes(e);
        const from = Math.max(0, Math.round((start.getTime() - calendar.weekStart.getTime()) / 86_400_000));
        const to = Math.min(7, Math.round((end.getTime() - calendar.weekStart.getTime()) / 86_400_000));
        return { e, from, to };
      })
      .filter((x) => x.to > x.from),
  );

  const title = $derived.by(() => {
    const first = days[0];
    const last = days[6];
    const month = (d: Date) => d.toLocaleDateString(undefined, { month: "long" });
    const range =
      first.getMonth() === last.getMonth()
        ? `${month(first)} ${first.getFullYear()}`
        : `${first.toLocaleDateString(undefined, { month: "short" })} – ${last.toLocaleDateString(undefined, { month: "short", year: "numeric" })}`;
    return range;
  });

  const time = (d: Date) => d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  const hourLabel = (h: number) => new Date(2000, 0, 1, h).toLocaleTimeString(undefined, { hour: "numeric" });

  const tone = (e: CalendarEvent) => TONE_CLASSES[eventTone(e)];

  function select(e: CalendarEvent) {
    calendar.selected = e;
    mail.overlay = "event";
  }

  onMount(() => {
    calendar.loadWeek();
    tick().then(() => scroller && (scroller.scrollTop = 7 * HOUR));
    const timer = setInterval(() => (now = new Date()), 60_000);
    return () => clearInterval(timer);
  });
</script>

<div data-testid="week-view" class="flex h-full min-w-0 flex-1 flex-col">
  <header class="flex h-14 shrink-0 items-center gap-2 border-b border-ink-800 px-4">
    <button data-testid="week-back" title="Back to inbox (Esc)" aria-label="Back to inbox" onclick={() => mail.back()} class="icon-btn"><ArrowLeft size={17} /></button>
    <h1 data-testid="week-title" class="ml-1 text-[17px] font-semibold text-ink-100">{title}</h1>
    <span class="ml-auto flex items-center gap-1">
      <button data-testid="week-new" title="New event (Shift+C)" aria-label="New event" onclick={() => (mail.overlay = "quickadd")} class="icon-btn"><Plus size={17} /></button>
      <button data-testid="week-prev" title="Previous week (←)" aria-label="Previous week" onclick={() => calendar.shiftWeek(-1)} class="icon-btn"><ChevronLeft size={17} /></button>
      <button data-testid="week-today" onclick={() => calendar.thisWeek()} disabled={calendar.isThisWeek} class="rounded-md px-2.5 py-1 text-sm text-ink-300 hover:bg-ink-800 hover:text-ink-100 disabled:opacity-40">Today</button>
      <button data-testid="week-next" title="Next week (→)" aria-label="Next week" onclick={() => calendar.shiftWeek(1)} class="icon-btn"><ChevronRight size={17} /></button>
    </span>
  </header>

  <!-- Day headers + all-day row share the grid's columns. Both reserve the scrollbar's
       width (scrollbar-gutter), so header and body columns line up. -->
  <div class="grid shrink-0 grid-cols-[3.5rem_repeat(7,minmax(0,1fr))] overflow-y-hidden border-b border-ink-800 [scrollbar-gutter:stable]">
    <span></span>
    {#each days as d (d.getTime())}
      {@const today = d.getTime() === todayMs}
      <div data-testid="week-day" data-today={today} class="border-l border-ink-800 px-2 py-2 text-center">
        <span class="block text-xs text-ink-500">{d.toLocaleDateString(undefined, { weekday: "short" })}</span>
        <span class={["mt-0.5 inline-grid size-7 place-items-center rounded-full text-sm", today ? "bg-accent font-semibold text-chrome" : "text-ink-100"]}>{d.getDate()}</span>
      </div>
    {/each}
    {#if allDay.length}
      <span class="py-1 pr-2 text-right text-[11px] text-ink-500">all day</span>
      <div class="relative col-span-7 grid grid-cols-7 gap-y-1 py-1">
        {#each allDay as { e, from, to } (e.id)}
          <button
            data-testid="week-all-day"
            data-remote-id={e.remoteId}
            data-tone={eventTone(e)}
            onclick={() => select(e)}
            style:grid-column="{from + 1} / {to + 1}"
            class="mx-0.5 truncate rounded px-2 py-0.5 text-left text-xs {tone(e)}">{#if mail.accounts.length > 1}<span data-testid="event-account" title={mail.accountOf(e.accountId)?.email} class={["mr-1 inline-block size-2 shrink-0 rounded-full align-middle", mail.accountColor(e.accountId)]}></span>{/if}{e.subject}</button
          >
        {/each}
      </div>
    {/if}
  </div>

  <div bind:this={scroller} class="min-h-0 flex-1 overflow-y-auto [scrollbar-gutter:stable]">
    <div class="relative grid grid-cols-[3.5rem_repeat(7,minmax(0,1fr))]" style:height="{24 * HOUR}px">
      <div class="relative">
        {#each Array.from({ length: 24 }, (_, h) => h) as h (h)}
          <span class="absolute right-2 -translate-y-2 text-[11px] text-ink-500" style:top="{h * HOUR}px">{h === 0 ? "" : hourLabel(h)}</span>
        {/each}
      </div>
      {#each days as d, di (d.getTime())}
        <div data-testid="week-column" class="relative border-l border-ink-800">
          {#each Array.from({ length: 24 }, (_, h) => h) as h (h)}
            <div class="absolute inset-x-0 border-t border-ink-800" style:top="{h * HOUR}px"></div>
          {/each}
          {#each timedByDay[di] as p (p.item.event.id)}
            {@const e = p.item.event}
            {@const px = (p.height / 60) * HOUR - 2}
            <button
              data-testid="week-event"
              data-remote-id={e.remoteId}
              data-response={e.response ?? "none"}
            data-tone={eventTone(e)}
              onclick={() => select(e)}
              title="{e.subject}{e.location ? ` · ${e.location}` : ''}"
              class={[
                "absolute overflow-hidden rounded-md px-1.5 text-left leading-tight",
                // Short events get one compact line instead of a clipped two-line block.
                px < 22 ? "flex items-center text-[10px]" : "py-0.5 text-[11px]",
                tone(e),
              ]}
              style:top="{(p.top / 60) * HOUR + 1}px"
              style:height="{px}px"
              style:left="calc({(p.col / p.cols) * 100}% + 2px)"
              style:width="calc({100 / p.cols}% - 4px)"
            >
              <JoinChip {e} {now} testid="week-join" compact={px < 16} />
              <span class="block truncate font-semibold">{#if mail.accounts.length > 1}<span data-testid="event-account" title={mail.accountOf(e.accountId)?.email} class={["mr-1 inline-block size-2 shrink-0 rounded-full align-middle", mail.accountColor(e.accountId)]}></span>{/if}{e.subject || "(no title)"}</span>
              {#if px >= 32}<span class="block truncate text-ink-300">{time(p.item.start)}</span>{/if}
            </button>
          {/each}
          {#if d.getTime() === todayMs}
            <div data-testid="week-now-line" class="pointer-events-none absolute inset-x-0 z-10 border-t-2 border-red-400" style:top="{((now.getTime() - todayMs) / 3_600_000) * HOUR}px"></div>
          {/if}
        </div>
      {/each}
    </div>
  </div>
</div>
