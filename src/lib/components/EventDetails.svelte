<script lang="ts">
  // Event details (click an event in the day or week view): everything Outlook shows. The
  // cached row renders at once; the description, repeat pattern, reminder and privacy are
  // fetched when it opens. Occurrences of a series say so up front, and answering one asks
  // whether you mean just it or the whole series. Esc or the backdrop closes it.
  import { onMount } from "svelte";
  import { Bell, CalendarClock, Check, CircleHelp, Clock, ExternalLink, Lock, MapPin, Pencil, Repeat, Tag, Trash2, Users, Video, X } from "@lucide/svelte";
  import { api, type Attendee, type EventDetail, type Rsvp } from "#lib/api.js";
  import { calendar, eventTimes } from "#lib/calendar.svelte.js";
  import { keymap } from "#lib/keys.svelte.js";
  import { mail } from "#lib/mail.svelte.js";
  import { senderName } from "#lib/format.js";
  import { describeRecurrence } from "#lib/recurrence.js";
  import HtmlView from "./HtmlView.svelte";

  const e = $derived(calendar.selected);
  /** Whose calendar it's on. */
  const account = $derived(e ? mail.accountOf(e.accountId) : null);

  // ----- full details -----
  let detail = $state<EventDetail | null>(null);
  let detailError = $state<string | null>(null);
  let loadedFor: string | null = null;
  $effect(() => {
    if (!e || loadedFor === `${e.accountId}:${e.remoteId}`) return;
    const key = `${e.accountId}:${e.remoteId}`;
    loadedFor = key;
    detail = null;
    showAllGuests = false;
    pending = null;
    deleting = null;
    detailError = null;
    // Placeholders for events still being created aren't on the server yet.
    if (e.remoteId.startsWith("local:")) return;
    api.eventDetail(e.accountId, e.remoteId).then(
      (d) => loadedFor === key && (detail = d),
      (err) => loadedFor === key && (detailError = String(err)),
    );
  });

  /** Part of a series: known from the row at once, confirmed by the details. */
  const inSeries = $derived(!!e && (e.eventType === "occurrence" || e.eventType === "exception" || !!detail?.isOccurrence));
  /** The series itself (a recurring invite's "Show in calendar"): answering, editing and deleting it cover every meeting. */
  const isMaster = $derived(e?.eventType === "seriesMaster");
  const description = $derived(detail && detail.bodyHtml.replace(/<p>\s*(<br\s*\/?>)?\s*<\/p>/g, "").trim() ? detail.bodyHtml : "");

  const when = $derived.by(() => {
    if (!e) return "";
    const { start, end } = eventTimes(e);
    const date = (d: Date) => d.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "long", year: start.getFullYear() === new Date().getFullYear() ? undefined : "numeric" });
    if (e.isAllDay) {
      const last = new Date(end.getTime() - 86_400_000);
      return last.getTime() === start.getTime() ? `${date(start)} · all day` : `${date(start)} – ${date(last)} · all day`;
    }
    const t = (d: Date) => d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    const sameDay = start.toDateString() === end.toDateString();
    return sameDay ? `${date(start)} · ${t(start)} – ${t(end)}` : `${date(start)} ${t(start)} – ${date(end)} ${t(end)}`;
  });

  const reminder = $derived.by(() => {
    if (!detail) return null;
    if (detail.reminderDefault) return "Default reminder";
    const m = detail.reminderMinutes;
    if (m === null) return "No reminder";
    if (m === 0) return "At the start";
    if (m % 1440 === 0) return `${m / 1440} ${m === 1440 ? "day" : "days"} before`;
    if (m % 60 === 0) return `${m / 60} ${m === 60 ? "hour" : "hours"} before`;
    return `${m} minutes before`;
  });

  /** The event's join link, or one found in its description. */
  const joinUrl = $derived(e?.joinUrl ?? detail?.joinUrl ?? null);
  const joinLabel = $derived.by(() => {
    const p = e?.joinUrl ? e.joinProvider : detail?.joinProvider;
    return !joinUrl ? "" : p === "Google Meet" ? "Google Meet" : p && p !== "Online meeting" ? `${p} meeting` : "Online meeting";
  });

  const SHOW_AS: Record<string, string> = { free: "Free", tentative: "Tentative", busy: "Busy", oof: "Out of office", workingElsewhere: "Working elsewhere" };

  // ----- attendees, grouped by answer -----
  const GROUPS: { key: string; label: string; match: (a: Attendee) => boolean }[] = [
    { key: "accepted", label: "Accepted", match: (a) => a.response === "accepted" },
    { key: "tentative", label: "Tentative", match: (a) => a.response === "tentativelyAccepted" },
    { key: "declined", label: "Declined", match: (a) => a.response === "declined" },
    { key: "none", label: "No response", match: (a) => !["accepted", "tentativelyAccepted", "declined"].includes(a.response ?? "") },
  ];
  const guests = $derived(e ? e.attendees.filter((a) => a.address?.toLowerCase() !== e.organizerAddress?.toLowerCase()) : []);
  const grouped = $derived(GROUPS.map((g) => ({ ...g, people: guests.filter(g.match) })).filter((g) => g.people.length));
  /** Long guest lists start collapsed to the first few. */
  const COLLAPSED_GUESTS = 5;
  let showAllGuests = $state(false);
  const collapsible = $derived(guests.length > COLLAPSED_GUESTS);
  /** What's listed: everyone, or the first few in answer order (accepted first). */
  const shownGroups = $derived.by(() => {
    if (!collapsible || showAllGuests) return grouped;
    let left = COLLAPSED_GUESTS;
    return grouped
      .map((g) => {
        const people = g.people.slice(0, Math.max(0, left));
        left -= people.length;
        return { ...g, people, total: g.people.length };
      })
      .filter((g) => g.people.length);
  });
  const summary = $derived(
    grouped.map((g) => `${g.people.length} ${g.key === "none" ? "no response" : g.label.toLowerCase()}`).join(", "),
  );

  // ----- answering -----
  const canRespond = $derived(!!e && !e.isOrganizer && !!e.response && !["organizer", "none"].includes(e.response) && !e.isCancelled);
  const RESPONSE: Record<string, string> = { accepted: "Accepted", tentativelyAccepted: "Tentative", declined: "Declined", notResponded: "Not answered yet" };
  /** Something waiting for "this occurrence or the series?". */
  type Pending = { kind: "rsvp"; r: Rsvp } | { kind: "edit" } | { kind: "delete" };
  let pending = $state<Pending | null>(null);

  function answer(r: Rsvp) {
    if (!canRespond) return;
    if (isMaster) void send(r, true);
    else if (inSeries) pending = { kind: "rsvp", r };
    else void send(r, false);
  }

  /** Runs what was waiting, for one occurrence or the whole series. */
  function resolve(series: boolean) {
    const p = pending;
    pending = null;
    if (!p) return;
    if (p.kind === "rsvp") void send(p.r, series);
    else if (p.kind === "edit") startEdit(series);
    else deleting = { series };
  }

  // ----- edit and delete -----
  const canEdit = $derived(!!e && !e.remoteId.startsWith("local:"));
  /** A delete waiting for confirmation (and, for meetings you organise, a message). */
  let deleting = $state<{ series: boolean } | null>(null);
  let deleteMessage = $state("");
  /** Cancelling a meeting tells its guests; your own events and others' just go. */
  const cancelsMeeting = $derived(!!e && e.isOrganizer && guests.length > 0);

  function edit() {
    if (!canEdit) return;
    if (isMaster) startEdit(true);
    else if (inSeries) pending = { kind: "edit" };
    else startEdit(false);
  }

  function startEdit(series: boolean) {
    if (!e) return;
    calendar.editing = { mode: "edit", event: e, series };
    mail.overlay = "editevent";
  }

  function askDelete() {
    if (!e) return;
    deleteMessage = "";
    if (isMaster) deleting = { series: true };
    else if (inSeries) pending = { kind: "delete" };
    else deleting = { series: false };
  }

  async function confirmDelete() {
    if (!e || !deleting) return;
    const target = deleting.series ? (e.seriesMasterId ?? detail?.seriesMasterId ?? e.remoteId) : e.remoteId;
    const what = deleting.series ? "series" : inSeries ? "occurrence" : "event";
    try {
      await api.deleteCalendarEvent(e.accountId, target, cancelsMeeting, cancelsMeeting ? deleteMessage : null);
      mail.showToast({ text: cancelsMeeting ? `Cancelled the meeting${what === "event" ? "" : ` ${what}`}; guests are told` : `Deleted the ${what}` });
      deleting = null;
      close();
      await calendar.refresh();
    } catch (err) {
      mail.showToast({ text: String(err), error: true });
    }
  }

  async function send(r: Rsvp, series: boolean) {
    if (!e) return;
    pending = null;
    const status = { accept: "accepted", tentative: "tentativelyAccepted", decline: "declined" }[r];
    try {
      await api.respondToEvent(e.accountId, e.remoteId, r, series);
      calendar.selected = { ...e, response: status };
      mail.showToast({ text: `${{ accept: "Accepted", tentative: "Tentatively accepted", decline: "Declined" }[r]}${inSeries || isMaster ? (series ? " the series" : " this occurrence") : ""}` });
      await calendar.refresh();
    } catch (err) {
      mail.showToast({ text: String(err), error: true });
    }
  }

  function close() {
    mail.overlay = null;
    calendar.selected = null;
  }

  onMount(() => {
    const open = () => mail.overlay === "event";
    return keymap.register([
      { id: "event.accept", keys: "y", label: "Accept", group: "Event", when: () => open() && canRespond, run: () => answer("accept") },
      { id: "event.maybe", keys: "m", label: "Maybe (tentative)", group: "Event", when: () => open() && canRespond, run: () => answer("tentative") },
      { id: "event.decline", keys: "n", label: "Decline", group: "Event", when: () => open() && canRespond, run: () => answer("decline") },
      { id: "event.scope.one", keys: "o", label: "This occurrence", group: "Event", when: () => open() && pending !== null, run: () => resolve(false) },
      { id: "event.scope.series", keys: "s", label: "The whole series", group: "Event", when: () => open() && pending !== null, run: () => resolve(true) },
      { id: "event.edit", keys: "e", label: "Edit event", group: "Event", when: () => open() && canEdit && !deleting, run: edit },
      { id: "event.delete", keys: "#", label: "Delete event", group: "Event", when: () => open() && !deleting, run: askDelete },
      { id: "event.delete.key", keys: "Delete", label: "Delete event", group: "Event", when: () => open() && !deleting, run: askDelete },
      { id: "event.delete.confirm", keys: "ctrl+Enter", label: "Confirm delete", group: "Event", when: () => open() && !!deleting, run: confirmDelete },
      { id: "event.join", keys: "j", label: "Join online meeting", group: "Event", when: () => open() && !!joinUrl, run: () => joinUrl && api.openExternal(joinUrl) },
    ]);
  });
</script>

{#if e}
  <div
    data-testid="event-details"
    onclick={(ev) => ev.target === ev.currentTarget && close()}
    role="presentation"
    class="fixed inset-x-0 top-9 bottom-0 z-30 grid place-items-center bg-black/50"
  >
    <!-- Capped to the window below the title bar (viewport units: a % max-height has no effect in a
         grid with an auto row). The header and buttons stay put; the details scroll. -->
    <div data-testid="event-panel" class="flex max-h-[calc(100vh-2.25rem-3rem)] w-[min(820px,calc(100vw-4rem))] flex-col rounded-xl border border-ink-700 bg-ink-900 shadow-2xl">
      <div class="shrink-0 p-5 pb-0">
        <div class="flex items-start gap-3">
          <h2 data-testid="event-title" class="flex-1 text-lg font-semibold text-ink-100 select-text">{e.subject || "(no title)"}</h2>
          <button data-testid="event-close" title="Close (Esc)" aria-label="Close" onclick={close} class="icon-btn size-7"><X size={16} /></button>
        </div>
        {#if inSeries || isMaster}
          <!-- Never leave you guessing whether this is one meeting or all of them. -->
          <div data-testid="event-series" class="mt-2 flex items-start gap-2 rounded-md border border-accent/40 bg-accent/10 px-3 py-2 text-sm">
            <Repeat size={15} class="mt-0.5 shrink-0 text-accent" />
            <div class="min-w-0">
              <p class="font-medium text-ink-100">
                {isMaster ? "Every meeting in a recurring series" : "One occurrence of a recurring series"}{#if detail?.isException}<span data-testid="event-exception" class="font-normal text-amber-300"> · changed from the series</span>{/if}
              </p>
              <p data-testid="event-recurrence" class="text-ink-300">
                {#if detail?.recurrence}{describeRecurrence(detail.recurrence)}{:else if detail}Repeats{:else}…{/if}
              </p>
            </div>
          </div>
        {/if}
      </div>

      <div data-testid="event-scroll" class="min-h-0 flex-1 overflow-y-auto px-5">
        <dl class="mt-3 space-y-2.5 text-sm text-ink-300">
          <div class="flex gap-3"><Clock size={16} class="mt-0.5 shrink-0 text-ink-500" /><span data-testid="event-when" class="text-ink-100">{when}</span></div>
          {#if account}
            <div data-testid="event-calendar" class="flex items-center gap-3">
              <span class="flex w-4 shrink-0 justify-center"><span class={["size-2.5 rounded-full", mail.accountColor(account.id)]}></span></span>
              <span>{account.provider === "google" ? "Google Calendar" : "Outlook calendar"} · {account.email}</span>
            </div>
          {/if}
          {#if e.location}
            <div class="flex gap-3"><MapPin size={16} class="mt-0.5 shrink-0 text-ink-500" /><span data-testid="event-location" class="select-text">{e.location}</span></div>
          {/if}
          {#if joinUrl}
            <div class="flex gap-3">
              <Video size={16} class="mt-0.5 shrink-0 text-ink-500" />
              <button data-testid="event-join-link" onclick={() => api.openExternal(joinUrl!)} class="truncate text-left text-accent hover:underline">
                {joinLabel}
              </button>
            </div>
          {/if}
          <div class="flex flex-wrap gap-x-5 gap-y-1">
            {#if e.showAs}
              <span data-testid="event-show-as" class="flex items-center gap-2"><CalendarClock size={16} class="text-ink-500" />{SHOW_AS[e.showAs] ?? e.showAs}</span>
            {/if}
            {#if reminder}
              <span data-testid="event-reminder" class="flex items-center gap-2"><Bell size={16} class="text-ink-500" />{reminder}</span>
            {/if}
            {#if detail && detail.sensitivity !== "normal"}
              <span data-testid="event-private" class="flex items-center gap-2"><Lock size={16} class="text-ink-500" />{detail.sensitivity === "confidential" ? "Confidential" : "Private"}</span>
            {/if}
          </div>
          {#if detail?.categories.length}
            <div data-testid="event-categories" class="flex flex-wrap items-center gap-2">
              <Tag size={16} class="shrink-0 text-ink-500" />
              {#each detail.categories as c (c)}<span class="rounded bg-ink-800 px-2 py-0.5 text-xs text-ink-100">{c}</span>{/each}
            </div>
          {/if}

          {#if e.attendees.length || e.organizerAddress}
            <div class="flex gap-3">
              <Users size={16} class="mt-0.5 shrink-0 text-ink-500" />
              <div class="min-w-0 flex-1 space-y-2">
                {#if e.organizerAddress}
                  <p data-testid="event-organizer">{senderName(e.organizerName, e.organizerAddress)} <span class="text-ink-500">· organiser{e.isOrganizer ? " (you)" : ""}</span></p>
                {/if}
                {#if guests.length}
                  <p data-testid="event-guest-count" class="text-ink-100">{guests.length} {guests.length === 1 ? "guest" : "guests"} <span class="text-ink-500">· {summary}</span></p>
                  <div data-testid="event-attendees" data-collapsed={collapsible && !showAllGuests} class="space-y-1.5">
                    {#each shownGroups as g (g.key)}
                      <div data-testid="event-attendee-group" data-group={g.key}>
                        <p class="text-xs tracking-wide text-ink-500 uppercase">{g.label} · {"total" in g ? g.total : g.people.length}</p>
                        <ul>
                          {#each g.people as a, i (a.address ?? i)}
                            <li class="truncate">{senderName(a.name, a.address)}{#if a.type === "optional"}<span class="text-ink-500">&nbsp;· optional</span>{/if}</li>
                          {/each}
                        </ul>
                      </div>
                    {/each}
                  </div>
                  {#if collapsible}
                    <button data-testid="event-attendees-expand" onclick={() => (showAllGuests = !showAllGuests)} class="text-xs text-accent hover:underline">
                      {showAllGuests ? "Show fewer" : `Show all ${guests.length} guests`}
                    </button>
                  {/if}
                {/if}
              </div>
            </div>
          {/if}
        </dl>

        <section class="mt-4 mb-1">
          {#if description}
            <HtmlView html={description} testid="event-description" />
          {:else if !detail && !detailError && !e.remoteId.startsWith("local:")}
            <div data-testid="event-description-loading" class="h-10 animate-pulse rounded bg-ink-800"></div>
          {:else if detailError && e.preview}
            <!-- Offline: the cached preview is better than nothing. -->
            <p data-testid="event-preview" class="text-sm whitespace-pre-line text-ink-300 select-text">{e.preview}</p>
          {/if}
        </section>

        {#if canRespond}
          <div data-testid="event-rsvp" class="mt-3 mb-1 space-y-2">
            <p data-testid="event-response" class="text-xs text-ink-500">Your response: {RESPONSE[e.response ?? ""] ?? e.response}</p>
            {#if pending?.kind === "rsvp"}
              <div data-testid="event-rsvp-scope" class="flex flex-wrap items-center gap-2 rounded-md border border-accent/40 bg-accent/10 px-3 py-2 text-sm">
                <span class="text-ink-100">{{ accept: "Accept", tentative: "Maybe", decline: "Decline" }[pending.r]}:</span>
                <button data-testid="rsvp-scope-one" onclick={() => resolve(false)} class="rounded border border-ink-700 px-2 py-1 hover:border-accent">This occurrence <kbd>o</kbd></button>
                <button data-testid="rsvp-scope-series" onclick={() => resolve(true)} class="rounded border border-ink-700 px-2 py-1 hover:border-accent">The whole series <kbd>s</kbd></button>
                <button onclick={() => (pending = null)} class="ml-auto text-xs text-ink-500 hover:text-ink-100">Cancel</button>
              </div>
            {:else}
              <div class="flex gap-2">
                {#each [{ r: "accept", label: "Accept", key: "y", icon: Check, on: "accepted" }, { r: "tentative", label: "Maybe", key: "m", icon: CircleHelp, on: "tentativelyAccepted" }, { r: "decline", label: "Decline", key: "n", icon: X, on: "declined" }] as b (b.r)}
                  <button
                    data-testid="event-rsvp-{b.r}"
                    onclick={() => answer(b.r as Rsvp)}
                    class={["flex items-center gap-1.5 rounded-md border px-3 py-1.5 text-sm", e.response === b.on ? "border-accent bg-accent/15 text-ink-100" : "border-ink-700 text-ink-300 hover:text-ink-100"]}
                  >
                    <b.icon size={14} /> {b.label} <kbd>{b.key}</kbd>
                  </button>
                {/each}
              </div>
            {/if}
          </div>
        {/if}
      </div>

      {#if pending && pending.kind !== "rsvp"}
        <div data-testid="event-scope" class="mx-5 mb-3 flex flex-wrap items-center gap-2 rounded-md border border-accent/40 bg-accent/10 px-3 py-2 text-sm">
          <span class="text-ink-100">{pending.kind === "edit" ? "Edit" : "Delete"}:</span>
          <button data-testid="scope-one" onclick={() => resolve(false)} class="rounded border border-ink-700 px-2 py-1 hover:border-accent">This occurrence <kbd>o</kbd></button>
          <button data-testid="scope-series" onclick={() => resolve(true)} class="rounded border border-ink-700 px-2 py-1 hover:border-accent">The whole series <kbd>s</kbd></button>
          <button onclick={() => (pending = null)} class="ml-auto text-xs text-ink-500 hover:text-ink-100">Cancel</button>
        </div>
      {/if}
      {#if deleting}
        <div data-testid="event-delete-confirm" class="mx-5 mb-3 space-y-2 rounded-md border border-red-900/60 bg-red-950/40 px-3 py-2 text-sm">
          <p class="text-ink-100">
            {#if cancelsMeeting}
              Cancel {deleting.series ? "every meeting in this series" : inSeries ? "this occurrence of the meeting" : "this meeting"}? Guests get a cancellation.
            {:else if !e.isOrganizer && guests.length}
              Remove {deleting.series ? "the whole series" : "this"} from your calendar? The organiser isn't told; decline instead to let them know.
            {:else}
              Delete {deleting.series ? "every event in this series" : inSeries ? "this occurrence" : "this event"}?
            {/if}
          </p>
          {#if cancelsMeeting}
            <textarea data-testid="event-delete-message" bind:value={deleteMessage} rows="2" placeholder="Message to guests (optional)" class="w-full rounded-md border border-ink-700 bg-ink-950 px-2 py-1 text-sm text-ink-100 outline-none focus:border-accent"></textarea>
          {/if}
          <div class="flex items-center gap-2">
            <button data-testid="event-delete-go" onclick={confirmDelete} class="rounded-md bg-red-500/90 px-3 py-1.5 text-sm font-medium text-white hover:bg-red-500">
              {cancelsMeeting ? "Cancel meeting" : !e.isOrganizer && guests.length ? "Remove" : "Delete"} <kbd class="opacity-70">Ctrl</kbd> <kbd class="opacity-70">Enter</kbd>
            </button>
            <button data-testid="event-delete-back" onclick={() => (deleting = null)} class="text-xs text-ink-500 hover:text-ink-100">Keep it</button>
          </div>
        </div>
      {/if}
      <div data-testid="event-actions" class="flex shrink-0 gap-2 border-t border-ink-800 p-5 pt-4">
        {#if canEdit}
          <button data-testid="event-edit" onclick={edit} class="flex items-center gap-2 rounded-md border border-ink-700 px-3 py-1.5 text-sm text-ink-300 hover:text-ink-100">
            <Pencil size={15} /> Edit <kbd>e</kbd>
          </button>
        {/if}
        {#if joinUrl}
          <button data-testid="event-join" onclick={() => api.openExternal(joinUrl!)} class="flex items-center gap-2 rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-chrome">
            <Video size={15} /> Join <kbd class="opacity-70">j</kbd>
          </button>
        {/if}
        {#if e.webLink}
          <button data-testid="event-open" onclick={() => api.openExternal(e.webLink!)} class="flex items-center gap-2 rounded-md border border-ink-700 px-3 py-1.5 text-sm text-ink-300 hover:text-ink-100">
            <ExternalLink size={15} /> {account?.provider === "google" ? "Open in Google Calendar" : "Open in Outlook"}
          </button>
        {/if}
        <button data-testid="event-delete" onclick={askDelete} title="Delete (#)" class="ml-auto flex items-center gap-2 rounded-md border border-ink-700 px-3 py-1.5 text-sm text-ink-300 hover:border-red-900 hover:text-red-300">
          <Trash2 size={15} /> {cancelsMeeting ? "Cancel meeting" : !e.isOrganizer && guests.length ? "Remove" : "Delete"}
        </button>
      </div>
    </div>
  </div>
{/if}
