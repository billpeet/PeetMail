<script lang="ts">
  // Event editor (create and edit), Outlook-style: title, calendar, all day, start/end,
  // repeat, location, guests, show as, reminder, private, and a rich-text description.
  // Editing loads the event fresh and sends only what changed. Recurring events say up
  // front whether you're editing one occurrence or the whole series. Guests of a meeting
  // can only change their own show-as and reminder. Ctrl+Enter saves; Esc closes (twice
  // when there are unsaved changes).
  import { onMount } from "svelte";
  import { DoorOpen, Repeat, Users, Video } from "@lucide/svelte";
  import { api, type Contact, type EditAttendee, type EditableEvent, type EventChanges, type EventTimes, type Room, type RoomList } from "#lib/api.js";
  import { calendar, localDate } from "#lib/calendar.svelte.js";
  import { keymap } from "#lib/keys.svelte.js";
  import { mail } from "#lib/mail.svelte.js";
  import { describeRecurrence, recurrenceProblem, sameRecurrence, type Recurrence } from "#lib/recurrence.js";
  import { senderName } from "#lib/format.js";
  import { narrow } from "#lib/contacts.js";
  import { latestOnly, STALE } from "#lib/latest.js";
  import RichText from "./RichText.svelte";
  import RepeatEditor from "./RepeatEditor.svelte";

  const req = calendar.editing!;
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const creating = req.mode === "create";
  /** Part of a series (when editing). */
  const inSeries = req.mode === "edit" && (req.event.eventType === "occurrence" || req.event.eventType === "exception" || req.series);
  const seriesScope = req.mode === "edit" && req.series;

  let loading = $state(!creating);
  let loadError = $state<string | null>(null);
  let original = $state.raw<EditableEvent | null>(null);

  // ----- fields -----
  const REMEMBER = "peetmail.quickAddAccount";
  const remembered = Number(localStorage.getItem(REMEMBER));
  let accountId = $state(
    req.mode === "edit"
      ? req.event.accountId
      : (req.accountId ?? (mail.accounts.some((a) => a.id === remembered) ? remembered : (mail.account?.id ?? 0))),
  );
  let subject = $state("");
  let allDay = $state(false);
  let startDate = $state("");
  let startTime = $state("09:00");
  let endDate = $state("");
  let endTime = $state("09:30");
  let location = $state("");
  let attendees = $state<EditAttendee[]>([]);
  let showAs = $state("busy");
  /** "none", "default" (Google's calendar default), or minutes. */
  let reminder = $state("15");
  let isPrivate = $state(false);
  let descriptionHtml = $state("");
  let descriptionDirty = $state(false);
  let recurrence = $state<Recurrence | null>(null);
  /** Add an online meeting (Teams, Meet...) when saving. */
  let online = $state(false);
  let isOrganizer = $state(true);
  let error = $state<string | null>(null);
  let saving = $state(false);
  let armedDiscard = $state(false);
  let titleInput: HTMLInputElement | undefined = $state();

  const guestMode = $derived(!isOrganizer);
  /** The pattern can change when creating, or for your own event or whole series (not one occurrence). */
  const canEditRepeat = $derived(creating || (!guestMode && (seriesScope || !inSeries)));
  const account = $derived(mail.accountOf(accountId));

  const pad = (n: number) => String(n).padStart(2, "0");
  const timeOf = (d: Date) => `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const addDaysIso = (iso: string, n: number) => {
    const [y, m, d] = iso.split("-").map(Number);
    return localDate(new Date(y, m - 1, d + n));
  };

  function setTimes(t: { start: Date; end: Date; allDay: boolean }) {
    allDay = t.allDay;
    startDate = localDate(t.start);
    startTime = timeOf(t.start);
    // All-day ends are exclusive on the wire; the form shows the last day.
    endDate = t.allDay ? localDate(new Date(t.end.getFullYear(), t.end.getMonth(), t.end.getDate() - 1)) : localDate(t.end);
    endTime = timeOf(t.end);
  }

  function fromWire(t: EventTimes): { start: Date; end: Date; allDay: boolean } {
    if (t.allDay) {
      const [ys, ms, ds] = t.start.split("-").map(Number);
      const [ye, me, de] = t.end.split("-").map(Number);
      return { start: new Date(ys, ms - 1, ds, 9), end: new Date(ye, me - 1, de, 9, 30), allDay: true };
    }
    return { start: new Date(t.start), end: new Date(t.end), allDay: false };
  }

  /** The form's times, as sent to the server. */
  function toWire(): EventTimes | string {
    if (allDay) {
      if (endDate < startDate) return "The end is before the start";
      return { start: startDate, end: addDaysIso(endDate, 1), allDay: true, timeZone: zone };
    }
    const s = new Date(`${startDate}T${startTime}`);
    const e = new Date(`${endDate}T${endTime}`);
    if (Number.isNaN(s.getTime()) || Number.isNaN(e.getTime())) return "Check the start and end";
    if (e <= s) return "The end is before the start";
    const iso = (d: Date) => d.toISOString().replace(/\.\d{3}Z$/, "Z");
    const t: EventTimes = { start: iso(s), end: iso(e), allDay: false, timeZone: zone };
    // A series repeats at its local time (see Times.local_start in event_edit.rs).
    if (recurrence) Object.assign(t, { localStart: `${startDate}T${startTime}:00`, localEnd: `${endDate}T${endTime}:00` });
    return t;
  }

  // Moving the start keeps the length.
  let lastStart = "";
  function onStartChange() {
    const key = `${startDate}T${startTime}`;
    if (lastStart && !allDay) {
      const before = new Date(lastStart).getTime();
      const after = new Date(key).getTime();
      const end = new Date(`${endDate}T${endTime}`).getTime();
      if (!Number.isNaN(before) && !Number.isNaN(after) && !Number.isNaN(end)) {
        const moved = new Date(end + (after - before));
        endDate = localDate(moved);
        endTime = timeOf(moved);
      }
    } else if (allDay && endDate < startDate) {
      endDate = startDate;
    }
    lastStart = key;
  }

  function reminderValue(minutes: number | null, isDefault: boolean) {
    return isDefault ? "default" : minutes === null ? "none" : String(minutes);
  }

  onMount(() => {
    if (req.mode === "create") {
      const p = req.prefill;
      if (p) {
        subject = p.subject;
        location = p.location ?? "";
        setTimes({ start: p.start, end: p.end, allDay: p.allDay });
      } else {
        // The next half hour, for 30 minutes.
        const now = new Date();
        const start = new Date(now.getFullYear(), now.getMonth(), now.getDate(), now.getHours(), now.getMinutes() < 30 ? 30 : 60);
        setTimes({ start, end: new Date(start.getTime() + 30 * 60_000), allDay: false });
      }
      lastStart = `${startDate}T${startTime}`;
      queueMicrotask(() => (req.focus === "location" ? locationInput : titleInput)?.focus());
    } else {
      api.eventForEdit(req.event.accountId, req.event.remoteId, req.series).then(
        (ev) => {
          original = ev;
          subject = ev.subject;
          setTimes(fromWire(ev.times));
          lastStart = `${startDate}T${startTime}`;
          location = ev.location;
          locationEmail = ev.locationEmail;
          attendees = ev.attendees.map((a) => ({ ...a }));
          showAs = ev.showAs;
          reminder = reminderValue(ev.reminderMinutes, ev.reminderDefault);
          isPrivate = ev.sensitivity === "private";
          descriptionHtml = ev.descriptionHtml;
          recurrence = ev.recurrence;
          online = ev.onlineMeeting;
          isOrganizer = ev.isOrganizer;
          loading = false;
          queueMicrotask(() => titleInput?.focus());
        },
        (e) => {
          loadError = String(e);
          loading = false;
        },
      );
    }
    return keymap.register([
      { id: "editor.save", keys: "ctrl+Enter", label: "Save event", group: "Event editor", when: () => mail.overlay === "editevent", run: save },
      { id: "editor.online", keys: "ctrl+m", label: "Add an online meeting (Teams, Meet)", group: "Event editor", when: () => mail.overlay === "editevent" && canAddOnline, run: () => (online = !online) },
      { id: "editor.close", keys: "Escape", label: "Close the editor", group: "Event editor", when: () => mail.overlay === "editevent" && !suggestions.length && !roomOpen, run: close },
    ]);
  });

  // ----- online meeting: what the account's calendar offers (Teams for work accounts) -----
  let meetingProvider = $state<string | null>(null);
  $effect(() => {
    const id = accountId;
    meetingProvider = null;
    // A new event's tick was for the last calendar's meeting kind; this one may offer another, or none.
    if (creating) online = false;
    api.meetingProvider(id).then((p) => accountId === id && (meetingProvider = p), () => {});
  });
  const hadOnline = $derived(!!original?.onlineMeeting);
  /** Once an event has one it stays: Outlook and Google don't take it off again. */
  /** Not on one occurrence: an online meeting belongs to the whole series. */
  const canAddOnline = $derived(!guestMode && !hadOnline && !!meetingProvider && !(inSeries && !seriesScope));
  const onlineLabel = $derived.by(() => {
    const p = meetingProvider ?? "";
    if (p === "hangoutsMeet") return "Google Meet";
    if (p.startsWith("teams")) return "Teams meeting";
    if (p.startsWith("skype")) return "Skype meeting";
    return "Online meeting";
  });

  // ----- rooms (Microsoft 365: suggested for the location, booked by inviting them) -----
  let locationEmail = $state<string | null>(null);
  let rooms = $state<Room[]>([]);
  let roomsNeed = $state<RoomList["needs"]>(null);
  let roomOpen = $state(false);
  // A list closes a moment after its field blurs (so a click on it still lands); coming
  // back before then cancels that, or it would shut on what you type next.
  let roomCloseTimer: ReturnType<typeof setTimeout> | undefined;
  let guestCloseTimer: ReturnType<typeof setTimeout> | undefined;
  let roomActive = $state(0);
  let granting = $state(false);
  let grantError = $state<string | null>(null);
  const NOT_NOW = "peetmail.roomsNotNow";
  let notNow = $state<number[]>(JSON.parse(localStorage.getItem(NOT_NOW) ?? "[]"));

  function loadRooms(id: number) {
    rooms = [];
    roomsNeed = null;
    if (mail.accountOf(id)?.provider !== "microsoft") return;
    api.listRooms(id).then((r) => {
      if (accountId !== id) return;
      rooms = r.rooms;
      roomsNeed = r.needs;
    }, () => {});
  }
  $effect(() => loadRooms(accountId));

  /** Reconnect the account with the permission rooms need (a browser sign-in), then look again. */
  async function grantRooms() {
    if (!roomsNeed) return;
    granting = true;
    grantError = null;
    try {
      await api.reconnectAccount(accountId, [roomsNeed.scope]);
      loadRooms(accountId);
      roomOpen = true;
      queueMicrotask(() => locationInput?.focus());
    } catch (e) {
      const msg = String(e);
      if (!msg.endsWith("cancelled")) grantError = /AADSTS90094|AADSTS65001|admin|access_denied/i.test(msg) ? "Your organisation needs an admin to approve this." : msg;
    } finally {
      granting = false;
    }
  }

  function roomsNotNow() {
    notNow = [...notNow, accountId];
    localStorage.setItem(NOT_NOW, JSON.stringify(notNow));
    roomOpen = false;
  }
  let locationInput = $state<HTMLInputElement>();
  /** The room booked as the location: shown by the location, not among the guests. */
  const locationRoom = $derived(locationEmail ? attendees.find((a) => a.resource && a.address.toLowerCase() === locationEmail?.toLowerCase()) : undefined);
  const guestChips = $derived(attendees.filter((a) => a !== locationRoom));
  const showRoomPermission = $derived(!guestMode && !rooms.length && !!roomsNeed && !notNow.includes(accountId) && (roomOpen || granting || !!grantError));
  /** Focusing shows every room; only what's typed since narrows them (not an existing location). */
  let typedSinceFocus = $state(false);
  let roomArrowed = $state(false);
  const roomMatches = $derived.by(() => {
    const q = typedSinceFocus ? location.trim().toLowerCase() : "";
    // The room already chosen isn't suggested again.
    const list = rooms.filter((r) => r.address.toLowerCase() !== locationEmail?.toLowerCase());
    if (!q) return list.slice(0, 8);
    return list.filter((r) => [r.name, r.address, r.building ?? ""].some((s) => s.toLowerCase().includes(q))).slice(0, 8);
  });
  const roomList = $derived(roomOpen && !guestMode ? roomMatches : []);

  /** Make the room the location and invite it (that's what books it), replacing any earlier room. */
  function pickRoom(r: Room) {
    const before = locationEmail?.toLowerCase();
    attendees = attendees.filter((a) => !(a.resource && a.address.toLowerCase() === before) && a.address.toLowerCase() !== r.address.toLowerCase());
    attendees = [...attendees, { name: r.name, address: r.address, optional: false, response: null, resource: true }];
    location = r.name;
    locationEmail = r.address;
    roomOpen = false;
    armedDiscard = false;
  }

  /** Typing over a chosen room un-books it. */
  function onLocationInput() {
    armedDiscard = false;
    roomOpen = true;
    typedSinceFocus = true;
    roomActive = 0;
    const room = attendees.find((a) => a.resource && a.address.toLowerCase() === locationEmail?.toLowerCase());
    if (locationEmail && location !== room?.name) {
      attendees = attendees.filter((a) => a !== room);
      locationEmail = null;
    }
  }

  function removeAttendee(a: EditAttendee) {
    attendees = attendees.filter((x) => x !== a);
    // Dropping the booked room clears it from the location too.
    if (a.resource && a.address.toLowerCase() === locationEmail?.toLowerCase()) {
      if (location === a.name) location = "";
      locationEmail = null;
    }
  }

  function locationKeys(e: KeyboardEvent) {
    if (!roomList.length) return;
    // Tab without picking: on to the next field, suggestions closed.
    if (e.key === "Tab" && !(typedSinceFocus || roomArrowed) ) {
      roomOpen = false;
      return;
    }
    // Enter/Tab pick only once you've typed or arrowed: tabbing past a location keeps it.
    const engaged = typedSinceFocus || roomArrowed;
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      if (roomArrowed) roomActive = (roomActive + (e.key === "ArrowDown" ? 1 : roomList.length - 1)) % roomList.length;
      roomArrowed = true;
    } else if (engaged && ((e.key === "Enter" && !e.ctrlKey) || (e.key === "Tab" && location.trim()))) {
      e.preventDefault();
      pickRoom(roomList[roomActive] ?? roomList[0]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      roomOpen = false;
    }
  }

  // ----- guests -----
  let guestText = $state("");
  let suggestions = $state<Contact[]>([]);
  let active = $state(0);
  const lookup = latestOnly(async (prefix: string) => (prefix ? api.suggestContacts(prefix) : []));

  async function suggest() {
    const text = guestText.trim();
    suggestions = text ? narrow(suggestions, text) : [];
    active = 0;
    const found = await lookup(text);
    if (found === STALE) return;
    const taken = new Set(attendees.map((a) => a.address.toLowerCase()));
    suggestions = found.filter((c) => !taken.has(c.address.toLowerCase()));
    active = 0;
  }

  function addGuest(c: { name: string | null; address: string }) {
    const address = c.address.trim();
    if (!address.includes("@") || attendees.some((a) => a.address.toLowerCase() === address.toLowerCase())) return;
    attendees = [...attendees, { name: c.name, address, optional: false, response: null }];
    guestText = "";
    suggestions = [];
    lookup.cancel();
  }

  /** "Name <a@b>" or "a@b". */
  function parseGuest(text: string): { name: string | null; address: string } | null {
    const m = text.match(/^\s*"?([^"<]*?)"?\s*<([^>]+)>\s*$/);
    if (m) return { name: m[1].trim() || null, address: m[2].trim() };
    const t = text.trim().replace(/[,;]$/, "");
    return t.includes("@") ? { name: null, address: t } : null;
  }

  function guestKeys(e: KeyboardEvent) {
    if (suggestions.length && (e.key === "ArrowDown" || e.key === "ArrowUp")) {
      e.preventDefault();
      active = (active + (e.key === "ArrowDown" ? 1 : suggestions.length - 1)) % suggestions.length;
    } else if (e.key === "Enter" || e.key === "Tab" || e.key === "," || e.key === ";") {
      if (e.key === "Enter" && e.ctrlKey) return; // save
      const pick = suggestions[active] ?? parseGuest(guestText);
      if (pick) {
        e.preventDefault();
        addGuest(pick);
      }
    } else if (e.key === "Escape" && suggestions.length) {
      e.preventDefault();
      e.stopPropagation();
      suggestions = [];
    } else if (e.key === "Backspace" && !guestText && guestChips.length) {
      removeAttendee(guestChips[guestChips.length - 1]);
    }
  }

  // ----- saving -----
  function changes(times: EventTimes): EventChanges {
    const c: EventChanges = {};
    const minutes = reminder === "none" ? null : reminder === "default" ? undefined : Number(reminder);
    if (!original) return c;
    if (subject !== original.subject) c.subject = subject.trim();
    const repeatChanged = canEditRepeat && !sameRecurrence(recurrence, original.recurrence);
    const plain = { start: times.start, end: times.end, allDay: times.allDay, timeZone: times.timeZone };
    const o = original.times;
    // A new or changed pattern goes with its times, so it repeats at the local time.
    if (JSON.stringify(plain) !== JSON.stringify({ start: o.start, end: o.end, allDay: o.allDay, timeZone: zone }) || (repeatChanged && recurrence)) c.times = times;
    if (repeatChanged) c.recurrence = recurrence;
    if (location !== original.location || locationEmail !== original.locationEmail) {
      c.location = location.trim();
      if (locationEmail) c.locationEmail = locationEmail;
    }
    if (descriptionDirty) c.descriptionHtml = descriptionHtml;
    const key = (list: EditAttendee[]) => JSON.stringify(list.map((a) => [a.address.toLowerCase(), a.optional, !!a.resource]));
    if (key(attendees) !== key(original.attendees)) c.attendees = attendees;
    if (showAs !== original.showAs) c.showAs = showAs;
    if (reminder !== reminderValue(original.reminderMinutes, original.reminderDefault) && minutes !== undefined) c.reminder = { minutes };
    if ((isPrivate ? "private" : "normal") !== original.sensitivity) c.sensitivity = isPrivate ? "private" : "normal";
    if (online && canAddOnline && meetingProvider) c.onlineMeeting = meetingProvider;
    return c;
  }

  const dirty = $derived.by(() => {
    if (creating) return subject.trim() !== "" || descriptionDirty || attendees.length > 0 || location !== "" || online;
    const t = toWire();
    return typeof t !== "string" && Object.keys(changes(t)).length > 0;
  });

  async function save() {
    if (saving || loading) return;
    error = null;
    if (!subject.trim()) {
      error = "Add a title";
      return;
    }
    const times = toWire();
    if (typeof times === "string") {
      error = times;
      return;
    }
    const repeatProblem = canEditRepeat && recurrence ? recurrenceProblem(recurrence) : null;
    if (repeatProblem) {
      error = repeatProblem;
      return;
    }
    saving = true;
    try {
      if (creating) {
        const minutes = reminder === "none" ? null : Number(reminder);
        await api.createEvent(accountId, {
          subject: subject.trim(),
          start: times.start,
          end: times.end,
          allDay: times.allDay,
          location: location.trim() || null,
          timeZone: zone,
          details: {
            ...(descriptionHtml ? { descriptionHtml } : {}),
            ...(locationEmail && location.trim() ? { locationEmail } : {}),
            ...(recurrence ? { recurrence, times } : {}),
            ...(attendees.length ? { attendees } : {}),
            ...(online && meetingProvider ? { onlineMeeting: meetingProvider } : {}),
            showAs,
            reminder: { minutes },
            sensitivity: isPrivate ? "private" : "normal",
          },
        });
        localStorage.setItem(REMEMBER, String(accountId));
        mail.showToast({ text: attendees.length ? `Sent “${subject.trim()}” to ${attendees.length} ${attendees.length === 1 ? "guest" : "guests"}` : `Created “${subject.trim()}”` });
        finish();
        await calendar.goToDay(fromWire(times).start);
      } else {
        const c = changes(times);
        if (Object.keys(c).length && original) {
          await api.updateEvent(accountId, original.remoteId, c);
          const told = isOrganizer && attendees.length > 0;
          mail.showToast({ text: told ? "Saved; guests will get an update" : "Saved" });
        }
        finish();
        await calendar.refresh();
      }
    } catch (e) {
      error = String(e);
    } finally {
      saving = false;
    }
  }

  function finish() {
    calendar.editing = null;
    calendar.selected = null;
    mail.overlay = null;
  }

  function close() {
    if (dirty && !armedDiscard) {
      armedDiscard = true;
      return;
    }
    finish();
  }

  const SHOW_AS = [
    ["busy", "Busy"],
    ["free", "Free"],
    ["tentative", "Tentative"],
    ["oof", "Out of office"],
    ["workingElsewhere", "Working elsewhere"],
  ];
  const REMINDERS = [
    ["none", "No reminder"],
    ["0", "At the start"],
    ["5", "5 minutes before"],
    ["10", "10 minutes before"],
    ["15", "15 minutes before"],
    ["30", "30 minutes before"],
    ["60", "1 hour before"],
    ["120", "2 hours before"],
    ["1440", "1 day before"],
    ["2880", "2 days before"],
    ["10080", "1 week before"],
  ];
  const field = "rounded-md border border-ink-700 bg-ink-950 px-2 py-1 text-sm text-ink-100 outline-none focus:border-accent disabled:opacity-50";
</script>

<div data-testid="event-editor" role="presentation" class="dialog-backdrop fixed inset-x-0 top-(--chrome-top) bottom-0 z-40 grid place-items-center bg-black/60">
  <div class="dialog-panel flex max-h-[calc(100vh-2.25rem-3rem)] w-[min(820px,calc(100vw-4rem))] flex-col rounded-xl border border-ink-700 bg-ink-900 shadow-2xl">
    <div class="shrink-0 space-y-2 p-5 pb-3">
      <div class="flex items-center justify-between text-xs tracking-wide text-ink-500 uppercase">
        <span data-testid="editor-mode">{creating ? "New event" : seriesScope ? "Edit series" : "Edit event"}</span>
        <span><kbd>Ctrl</kbd> <kbd>Enter</kbd> save · <kbd>Esc</kbd> close</span>
      </div>
      {#if inSeries}
        <!-- Never ambiguous: one meeting, or all of them. -->
        <div data-testid="editor-scope" data-series={seriesScope} class={["flex items-start gap-2 rounded-md border px-3 py-2 text-sm", seriesScope ? "border-amber-400/50 bg-amber-400/10" : "border-accent/40 bg-accent/10"]}>
          <Repeat size={15} class={["mt-0.5 shrink-0", seriesScope ? "text-amber-300" : "text-accent"]} />
          <div>
            <p class="font-medium text-ink-100">{seriesScope ? "Editing the whole series" : "Editing this occurrence only"}</p>
            <p class="text-ink-300">{seriesScope ? (recurrence ? describeRecurrence(recurrence) : "Every occurrence changes") : "The rest of the series stays as it is"}</p>
          </div>
        </div>
      {/if}
      {#if guestMode && !loading}
        <p data-testid="editor-guest" class="flex items-center gap-2 rounded-md bg-ink-800 px-3 py-2 text-sm text-ink-300">
          <Users size={15} class="shrink-0" /> You're a guest: you can change your show-as and reminder; the organiser owns the rest.
        </p>
      {/if}
      <input
        data-testid="editor-title"
        bind:this={titleInput}
        bind:value={subject}
        oninput={() => (armedDiscard = false)}
        disabled={loading || guestMode}
        placeholder="Add a title"
        class="w-full bg-transparent text-xl font-semibold text-ink-100 outline-none placeholder:text-ink-500 disabled:opacity-80"
      />
    </div>

    <div class="min-h-0 flex-1 overflow-y-auto px-5 pb-3">
      {#if loading}
        <div data-testid="editor-loading" class="h-40 animate-pulse rounded bg-ink-800"></div>
      {:else if loadError}
        <p data-testid="editor-error" class="text-sm text-red-300">Couldn't load the event: {loadError}</p>
      {:else}
        <div class="grid grid-cols-[7rem_1fr] items-center gap-x-3 gap-y-3 text-sm text-ink-300">
          {#if creating && mail.accounts.length > 1}
            <span>Calendar</span>
            <span class="flex items-center gap-2">
              <span class={["size-2.5 rounded-full", mail.accountColor(accountId)]}></span>
              <select data-testid="editor-calendar" bind:value={accountId} class={field}>
                {#each mail.accounts as a (a.id)}
                  <option value={a.id}>{a.provider === "google" ? "Google Calendar" : "Outlook calendar"} · {a.email}</option>
                {/each}
              </select>
            </span>
          {:else if account}
            <span>Calendar</span>
            <span data-testid="editor-calendar-fixed" class="flex items-center gap-2"><span class={["size-2.5 rounded-full", mail.accountColor(accountId)]}></span>{account.provider === "google" ? "Google Calendar" : "Outlook calendar"} · {account.email}</span>
          {/if}

          <span>Starts</span>
          <span class="flex flex-wrap items-center gap-2">
            <input data-testid="editor-start-date" type="date" bind:value={startDate} onchange={onStartChange} disabled={guestMode} class={field} />
            {#if !allDay}<input data-testid="editor-start-time" type="time" step="300" bind:value={startTime} onchange={onStartChange} disabled={guestMode} class={field} />{/if}
            <label class="ml-2 flex items-center gap-1.5">
              <input data-testid="editor-all-day" type="checkbox" bind:checked={allDay} disabled={guestMode} class="accent-accent" /> All day
            </label>
          </span>
          <span>Ends</span>
          <span class="flex flex-wrap items-center gap-2">
            <input data-testid="editor-end-date" type="date" bind:value={endDate} disabled={guestMode} class={field} />
            {#if !allDay}<input data-testid="editor-end-time" type="time" step="300" bind:value={endTime} disabled={guestMode} class={field} />{/if}
            <span class="text-xs text-ink-500">{zone}</span>
          </span>

          <span class={canEditRepeat ? "self-start pt-1.5" : ""}>Repeat</span>
          {#if canEditRepeat}
            <div>
              <RepeatEditor bind:value={recurrence} start={startDate} />
              {#if seriesScope && original?.recurrence && !recurrence}
                <p data-testid="repeat-stop-warning" class="mt-1 text-xs text-amber-300">Saving keeps only the first meeting; the other occurrences are removed{attendees.length ? " and guests are told" : ""}.</p>
              {:else if seriesScope && original?.recurrence && recurrence && !sameRecurrence(recurrence, original.recurrence)}
                <p data-testid="repeat-change-note" class="mt-1 text-xs text-ink-500">Changing the pattern resets occurrences you changed one at a time.</p>
              {/if}
            </div>
          {:else}
            <span data-testid="editor-repeat" class="text-ink-300">
              {#if recurrence}{describeRecurrence(recurrence)}{:else if inSeries && !seriesScope}Part of a series{:else}Doesn't repeat{/if}
            </span>
          {/if}

          <span>Location</span>
          <div class="relative">
            <div class="flex items-center gap-2">
              <input
                data-testid="editor-location"
                bind:this={locationInput}
                bind:value={location}
                oninput={onLocationInput}
                onfocus={() => {
                  clearTimeout(roomCloseTimer);
                  roomOpen = true;
                  typedSinceFocus = false;
                  roomArrowed = false;
                }}
                onblur={() => (roomCloseTimer = setTimeout(() => (roomOpen = false), 150))}
                onkeydown={locationKeys}
                disabled={guestMode}
                placeholder={rooms.length ? "Add a room, a place or a meeting link" : "Add a place or a meeting link"}
                autocomplete="off"
                class={[field, "flex-1"]}
              />
              {#if locationEmail}
                <span data-testid="editor-location-room" data-address={locationEmail} class="flex shrink-0 items-center gap-1 text-xs text-ink-500" title="Booked: {locationEmail}">
                  <DoorOpen size={13} /> Room booked
                  {#if locationRoom && !guestMode}
                    <button type="button" tabindex="-1" data-testid="editor-location-room-remove" title="Don't book this room" aria-label="Don't book this room" onclick={() => locationRoom && removeAttendee(locationRoom)} class="text-ink-500 hover:text-red-300">×</button>
                  {/if}
                </span>
              {/if}
            </div>
            {#if showRoomPermission && roomsNeed}
              <!-- Some tenants only list rooms once the account grants a directory permission. -->
              <div data-testid="editor-rooms-permission" role="presentation" onmousedown={(e) => e.preventDefault()} class="absolute top-full left-0 z-20 mt-1 w-[26rem] space-y-2 rounded-md border border-ink-700 bg-ink-900 p-3 text-sm shadow-xl">
                <p class="flex items-center gap-2 text-ink-100"><DoorOpen size={14} class="shrink-0 text-ink-500" /> Room suggestions</p>
                {#if grantError}
                  <p data-testid="editor-rooms-error" class="text-xs text-red-300">{grantError}</p>
                {:else if roomsNeed.admin}
                  <p class="text-xs text-ink-300">Your organisation only shares its room list with apps an admin has approved. You can ask for approval from the sign-in page.</p>
                {:else}
                  <p class="text-xs text-ink-300">To list your organisation's rooms, PeetMail needs permission to read basic directory info. You'll sign in again in your browser; your mail stays as it is.</p>
                {/if}
                <div class="flex items-center gap-3">
                  {#if granting}
                    <span data-testid="editor-rooms-granting" class="text-xs text-ink-300">Finish in your browser…</span>
                    <button type="button" onclick={() => api.cancelSignIn()} class="text-xs text-ink-500 hover:text-ink-100">Cancel</button>
                  {:else}
                    <button type="button" data-testid="editor-rooms-grant" onclick={grantRooms} class="rounded-md bg-accent px-2.5 py-1 text-xs font-medium text-white hover:brightness-110">
                      {roomsNeed.admin ? "Request admin approval" : "Allow room suggestions"}
                    </button>
                    <button type="button" data-testid="editor-rooms-not-now" onclick={roomsNotNow} class="text-xs text-ink-500 hover:text-ink-100">Not now</button>
                  {/if}
                </div>
              </div>
            {:else if roomList.length}
              <ul data-testid="editor-room-suggestions" class="absolute top-full left-0 z-20 mt-1 max-h-72 w-96 overflow-y-auto rounded-md border border-ink-700 bg-ink-900 shadow-xl">
                <li class="px-3 pt-1.5 pb-1 text-[11px] tracking-wide text-ink-500 uppercase">Rooms</li>
                {#each roomList as r, i (r.address)}
                  <li>
                    <button
                      type="button"
                      tabindex="-1"
                      data-testid="editor-room"
                      data-address={r.address}
                      onmousedown={(e) => {
                        e.preventDefault();
                        pickRoom(r);
                      }}
                      class={["flex w-full items-center gap-2 px-3 py-1.5 text-left text-sm", i === roomActive && (typedSinceFocus || roomArrowed) ? "bg-ink-800 text-ink-100" : "text-ink-300 hover:bg-ink-800/60"]}
                    >
                      <DoorOpen size={14} class="shrink-0 text-ink-500" />
                      <span class="min-w-0 flex-1 truncate">{r.name}</span>
                      <span class="shrink-0 text-xs text-ink-500">{[r.building, r.capacity ? `${r.capacity} seats` : null].filter(Boolean).join(" · ")}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>

          {#if canAddOnline || hadOnline}
            <span>Online</span>
            <span class="flex items-center gap-2">
              <label data-testid="editor-online" class="flex items-center gap-1.5" title="Ctrl+M">
                <input data-testid="editor-online-meeting" type="checkbox" tabindex="-1" bind:checked={online} onchange={() => (armedDiscard = false)} disabled={!canAddOnline} class="accent-accent" />
                <Video size={14} class="text-ink-500" />
                {hadOnline ? "Has an online meeting" : onlineLabel}
              </label>
              {#if canAddOnline}<kbd class="text-[11px]">Ctrl</kbd><kbd class="text-[11px]">M</kbd>{/if}
              {#if online && !hadOnline}<span data-testid="editor-online-note" class="text-xs text-ink-500">The join link is added when it's saved</span>{/if}
            </span>
          {/if}

          <span class="self-start pt-1.5">Guests</span>
          <div class="relative">
            <div data-testid="editor-attendees" class={["flex min-h-9 flex-wrap items-center gap-1.5 rounded-md border border-ink-700 bg-ink-950 px-2 py-1 focus-within:border-accent", guestMode && "opacity-50"]}>
              {#each guestChips as a (a.address)}
                <span data-testid="editor-attendee" data-address={a.address} data-room={a.resource ? "true" : undefined} class="flex items-center gap-1 rounded bg-ink-800 px-2 py-0.5 text-xs text-ink-100">
                  {#if a.resource}<DoorOpen size={12} class="text-ink-500" />{/if}
                  {senderName(a.name, a.address)}
                  {#if a.resource}<span class="text-ink-500">room</span>{/if}
                  {#if !guestMode}
                    {#if !a.resource}
                      <button type="button" tabindex="-1" data-testid="editor-attendee-optional" title="Required or optional" onclick={() => (attendees = attendees.map((x) => (x === a ? { ...a, optional: !a.optional } : x)))} class="text-ink-500 hover:text-ink-100">{a.optional ? "optional" : "required"}</button>
                    {/if}
                    <button type="button" tabindex="-1" data-testid="editor-attendee-remove" title="Remove" aria-label="Remove {a.address}" onclick={() => removeAttendee(a)} class="text-ink-500 hover:text-red-300">×</button>
                  {:else if a.optional}<span class="text-ink-500">optional</span>{/if}
                </span>
              {/each}
              {#if !guestMode}
                <input
                  data-testid="editor-attendee-input"
                  bind:value={guestText}
                  oninput={suggest}
                  onkeydown={guestKeys}
                  onfocus={() => clearTimeout(guestCloseTimer)}
                  onblur={() => (guestCloseTimer = setTimeout(() => (suggestions = []), 150))}
                  placeholder={attendees.length ? "" : "Invite people (name or address)"}
                  spellcheck="false"
                  autocomplete="off"
                  class="min-w-40 flex-1 bg-transparent py-0.5 text-sm text-ink-100 outline-none"
                />
              {/if}
            </div>
            {#if suggestions.length}
              <ul data-testid="editor-attendee-suggestions" class="absolute top-full left-0 z-10 mt-1 w-80 overflow-hidden rounded-md border border-ink-700 bg-ink-900 shadow-xl">
                {#each suggestions as s, i (s.address)}
                  <li>
                    <button type="button" tabindex="-1" onmousedown={(e) => { e.preventDefault(); addGuest(s); }} class={["block w-full px-3 py-1.5 text-left text-sm", i === active ? "bg-ink-800 text-ink-100" : "text-ink-300"]}>
                      {s.name ?? s.address} <span class="text-ink-500">{s.name ? s.address : ""}</span>
                    </button>
                  </li>
                {/each}
              </ul>
            {/if}
          </div>

          <span class="self-start pt-2">Description</span>
          <RichText
            html={descriptionHtml}
            placeholder="Add an agenda, notes or links"
            disabled={guestMode}
            onsubmit={save}
            onchange={(h) => {
              descriptionHtml = h;
              descriptionDirty = true;
              armedDiscard = false;
            }}
          />

          <span>Show as</span>
          <span class="flex flex-wrap items-center gap-4">
            <select data-testid="editor-show-as" bind:value={showAs} class={field}>
              {#each SHOW_AS as [v, label] (v)}<option value={v}>{label}</option>{/each}
            </select>
            <span class="flex items-center gap-2">
              Reminder
              <select data-testid="editor-reminder" bind:value={reminder} class={field}>
                {#if reminder === "default"}<option value="default">Calendar default</option>{/if}
                {#each REMINDERS as [v, label] (v)}<option value={v}>{label}</option>{/each}
              </select>
            </span>
            <label class="flex items-center gap-1.5">
              <input data-testid="editor-private" type="checkbox" bind:checked={isPrivate} disabled={guestMode} class="accent-accent" /> Private
            </label>
          </span>
        </div>
      {/if}
    </div>

    <div class="flex shrink-0 items-center gap-3 border-t border-ink-800 px-5 py-3">
      {#if error}
        <p data-testid="editor-error" class="text-sm text-red-300">{error}</p>
      {:else if armedDiscard}
        <p data-testid="editor-discard" class="text-sm text-amber-300">Unsaved changes. Press Esc again to discard.</p>
      {:else if isOrganizer && attendees.length && !creating}
        <p class="text-xs text-ink-500">Guests will get an update when you save.</p>
      {:else if creating && attendees.length}
        <p class="text-xs text-ink-500">Saving sends the invitation.</p>
      {/if}
      <span class="ml-auto flex gap-2">
        <button data-testid="editor-cancel" onclick={close} class="rounded-md border border-ink-700 px-3 py-1.5 text-sm text-ink-300 hover:text-ink-100">Cancel</button>
        <button
          data-testid="editor-save"
          onclick={save}
          disabled={saving || loading || !!loadError}
          title="Ctrl+Enter"
          class="flex items-center gap-2 rounded-md bg-accent px-3 py-1.5 text-sm font-medium text-chrome disabled:opacity-50"
        >
          <span data-testid="editor-save-label">{creating && attendees.length ? "Send" : "Save"}</span>
          <span class="flex gap-0.5 text-[11px] font-normal opacity-70"><kbd class="!border-chrome/30 !bg-transparent !text-chrome">Ctrl</kbd><kbd class="!border-chrome/30 !bg-transparent !text-chrome">Enter</kbd></span>
        </button>
      </span>
    </div>
  </div>
</div>
