<script lang="ts">
  // Quick add (Shift+C): type an event in plain words, see how it's understood, Enter creates it.
  import { onMount } from "svelte";
  import { CalendarPlus } from "@lucide/svelte";
  import { api } from "#lib/api.js";
  import { calendar, localDate } from "#lib/calendar.svelte.js";
  import { keymap } from "#lib/keys.svelte.js";
  import { mail } from "#lib/mail.svelte.js";
  import { parseQuickEvent } from "#lib/quickAdd.js";

  let text = $state("");
  let busy = $state(false);

  // Which account's calendar it goes on: the last one used (remembered), else the first.
  const REMEMBER = "peetmail.quickAddAccount";
  const remembered = Number(localStorage.getItem(REMEMBER));
  let accountId = $state(mail.accounts.some((a) => a.id === remembered) ? remembered : (mail.account?.id ?? 0));
  const target = $derived(mail.accountOf(accountId));

  function cycleAccount(step: number) {
    const list = mail.accounts;
    if (list.length < 2) return;
    const i = list.findIndex((a) => a.id === accountId);
    accountId = list[(i + step + list.length) % list.length].id;
    localStorage.setItem(REMEMBER, String(accountId));
  }
  let input: HTMLInputElement | undefined = $state();

  const parsed = $derived(text.trim() ? parseQuickEvent(text) : null);

  const preview = $derived.by(() => {
    if (!parsed?.ok) return null;
    const e = parsed.event;
    const day = (d: Date) => d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" });
    const t = (d: Date) => d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
    let when: string;
    if (e.allDay) {
      const last = new Date(e.end.getFullYear(), e.end.getMonth(), e.end.getDate() - 1);
      when = last.getTime() === e.start.getTime() ? `${day(e.start)} · all day` : `${day(e.start)} – ${day(last)} · all day`;
    } else {
      when = `${day(e.start)} · ${t(e.start)} – ${t(e.end)}`;
    }
    return { title: e.title, when, location: e.location };
  });

  async function create() {
    if (!parsed?.ok || !target || busy) return;
    const e = parsed.event;
    busy = true;
    try {
      await api.createEvent(target.id, {
        subject: e.title,
        start: e.allDay ? localDate(e.start) : e.start.toISOString().replace(/\.\d{3}Z$/, "Z"),
        end: e.allDay ? localDate(e.end) : e.end.toISOString().replace(/\.\d{3}Z$/, "Z"),
        allDay: e.allDay,
        location: e.location,
        timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      });
      mail.overlay = null;
      mail.showToast({ text: `Created “${e.title}”` });
      // Show where it landed.
      await calendar.goToDay(e.start);
      if (mail.view === "calendar") await calendar.showWeekOf(e.start);
    } catch (err) {
      mail.showToast({ text: String(err), error: true });
    } finally {
      busy = false;
    }
  }

  /** More than a title and a time: the full editor, prefilled with what's typed so far. */
  function moreOptions() {
    const ev = parsed?.ok ? parsed.event : null;
    calendar.editing = {
      mode: "create",
      accountId: target?.id ?? null,
      prefill: ev ? { subject: ev.title, start: ev.start, end: ev.end, allDay: ev.allDay, location: ev.location } : text.trim() ? { subject: text.trim(), ...nextSlot(), allDay: false, location: null } : undefined,
      // Title and time understood: carry on from the location.
      focus: ev ? "location" : "title",
    };
    mail.overlay = "editevent";
  }

  function nextSlot() {
    const now = new Date();
    const start = new Date(now.getFullYear(), now.getMonth(), now.getDate(), now.getHours(), now.getMinutes() < 30 ? 30 : 60);
    return { start, end: new Date(start.getTime() + 30 * 60_000) };
  }

  // The input handles its keys itself (the keymap ignores plain keys in text fields); the
  // bindings below list them in the ? overlay and work if focus is elsewhere in the box.
  function onkeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.ctrlKey) {
      e.preventDefault();
      create();
    } else if (e.key === "Tab" && e.ctrlKey) {
      // Ctrl+Tab: the next calendar (Ctrl+Shift+Tab: the previous one).
      e.preventDefault();
      cycleAccount(e.shiftKey ? -1 : 1);
    } else if (e.key === "Tab" && !e.shiftKey) {
      // Tab moves on to the full form: guests, location, description...
      e.preventDefault();
      moreOptions();
    } else if (e.key === "Tab") {
      e.preventDefault(); // nowhere to go back to
    }
  }

  onMount(() => {
    input?.focus();
    const open = () => mail.overlay === "quickadd";
    return keymap.register([
      { id: "quickadd.create", keys: "Enter", label: "Create the event", group: "Quick add", when: open, run: create },
      { id: "quickadd.more", keys: "Tab", label: "More options (the full editor)", group: "Quick add", when: open, run: moreOptions },
      { id: "quickadd.calendar", keys: "ctrl+Tab", label: "Next calendar", group: "Quick add", when: () => open() && mail.accounts.length > 1, run: () => cycleAccount(1) },
      { id: "quickadd.calendarPrev", keys: "ctrl+shift+Tab", label: "Previous calendar", group: "Quick add", when: () => open() && mail.accounts.length > 1, run: () => cycleAccount(-1) },
    ]);
  });
</script>

<div
  data-testid="quick-add"
  onclick={(e) => e.target === e.currentTarget && (mail.overlay = null)}
  role="presentation"
  class="dialog-backdrop fixed inset-x-0 top-(--chrome-top) bottom-0 z-30 flex justify-center bg-black/50 pt-[15vh]"
>
  <div class="dialog-sheet h-fit w-[560px] overflow-hidden rounded-xl border border-ink-700 bg-ink-900 shadow-2xl">
    <div class="flex items-center gap-3 border-b border-ink-800 px-4">
      <CalendarPlus size={18} class="shrink-0 text-accent" />
      <input
        data-testid="quick-add-input"
        bind:this={input}
        bind:value={text}
        {onkeydown}
        placeholder="Lunch with Sam fri 1pm @ Café Beetroot"
        spellcheck="false"
        autocomplete="off"
        class="w-full bg-transparent py-3 text-sm text-ink-100 outline-none select-text"
      />
    </div>
    <div data-testid="quick-add-preview" class="min-h-14 px-4 py-3 text-sm">
      {#if preview}
        <p data-testid="quick-add-title" class="font-semibold text-ink-100">{preview.title}</p>
        <p data-testid="quick-add-when" class="mt-0.5 text-ink-300">{preview.when}{preview.location ? ` · ${preview.location}` : ""}</p>
      {:else if parsed && !parsed.ok}
        <p data-testid="quick-add-hint" class="text-ink-500">{parsed.reason}</p>
      {:else}
        <p class="text-ink-500">Try “standup tomorrow 9:30 15m”, “dentist 3 nov 2pm”, “offsite fri all day”.</p>
      {/if}
    </div>
    {#if mail.accounts.length > 1 && target}
      <button
        data-testid="quick-add-account"
        data-account-id={target.id}
        onclick={() => cycleAccount(1)}
        title="Change calendar (Ctrl+Tab)"
        class="flex w-full items-center gap-2 border-t border-ink-800 px-4 py-2 text-left text-xs text-ink-300 hover:text-ink-100"
      >
        <span class="text-ink-500">Add to</span>
        <span class={["size-2 rounded-full", mail.accountColor(target.id)]}></span>
        <span>{target.provider === "google" ? "Google Calendar" : "Outlook calendar"} · {target.email}</span>
        <span class="ml-auto text-ink-500"><kbd>Ctrl</kbd> <kbd>Tab</kbd> change</span>
      </button>
    {/if}
    <div class="flex items-center justify-between border-t border-ink-800 px-4 py-2 text-xs text-ink-500">
      <span><kbd>Enter</kbd> create · <button data-testid="quick-add-more" onclick={moreOptions} class="hover:text-ink-100"><kbd>Tab</kbd> more options</button> · <kbd>Esc</kbd> cancel</span>
      <span>Adds to your calendar; no invitations are sent.</span>
    </div>
  </div>
</div>
