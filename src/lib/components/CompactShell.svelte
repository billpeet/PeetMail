<script lang="ts">
  // The phone layout (layout.svelte.ts, docs/ANDROID.md Stage 2): one screen at a time.
  // The list, or the conversation full screen, or the calendar's day; a bottom bar to switch
  // between mail, calendar, search and settings; the mailboxes in a drawer. Every control
  // calls what a keymap binding calls, so the keyboard (DeX, a Chromebook) still works.
  import { tick } from "svelte";
  import { AlertCircle, Archive, CalendarDays, LoaderCircle, Mail as MailIcon, MailOpen, Menu, AlarmClock, Search, Settings as SettingsIcon, SquarePen, Trash2, X, CircleHelp } from "@lucide/svelte";
  import { mail } from "#lib/mail.svelte.js";
  import { calendar } from "#lib/calendar.svelte.js";
  import { count } from "#lib/format.js";
  import { MAILBOXES, MAILBOX_TITLES } from "#lib/mailboxes.js";
  import ThreadList from "./ThreadList.svelte";
  import Reader from "./Reader.svelte";
  import ScheduledList from "./ScheduledList.svelte";
  import DraftsList from "./DraftsList.svelte";
  import DayView from "./DayView.svelte";

  let drawer = $state(false);
  let searchInput: HTMLInputElement | undefined = $state();

  const inCalendar = $derived(mail.view === "calendar");
  const inReader = $derived(mail.view === "reader");
  const searching = $derived(mail.search !== null);
  const special = $derived(!searching && (mail.mailbox === "scheduled" || mail.mailbox === "drafts"));
  const title = $derived(inCalendar ? "Calendar" : MAILBOX_TITLES[mail.mailbox]);
  const inboxUnread = $derived(mail.splits.some((s) => s.unread > 0));

  // The search box takes the keyboard as soon as search starts (bottom bar, `/`).
  let wasSearching = false;
  $effect(() => {
    if (searching && !wasSearching) tick().then(() => searchInput?.focus());
    wasSearching = searching;
  });

  function startSearch() {
    mail.showMail();
    mail.startSearch();
  }

  function goTo(id: (typeof MAILBOXES)[number]["id"]) {
    drawer = false;
    if (mail.search !== null) mail.endSearch();
    mail.showMail();
    mail.goTo(id);
  }

  function searchKeys(e: KeyboardEvent) {
    if (e.key === "Enter") {
      e.preventDefault();
      searchInput?.blur();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      searchInput?.blur();
      mail.endSearch();
    }
  }

  /** Esc (and Android's back) closes the drawer before anything behind it. */
  function closeDrawerFirst(e: KeyboardEvent) {
    if (e.key !== "Escape" || !drawer) return;
    e.preventDefault();
    e.stopPropagation();
    drawer = false;
  }

  const syncTitle = $derived(mail.sync.state === "syncing" ? "Syncing…" : mail.sync.state === "error" ? `Sync error: ${mail.sync.message ?? ""}` : "Up to date");
</script>

<svelte:window onkeydowncapture={closeDrawerFirst} />

{#snippet navButton(testid: string, label: string, Icon: typeof MailIcon, active: boolean, run: () => void, dot = false)}
  <button data-testid={testid} data-active={active} aria-current={active ? "page" : undefined} onclick={run} class={["relative flex flex-1 flex-col items-center gap-0.5 py-2 text-[11px]", active ? "text-ink-100" : "text-ink-500"]}>
    <span class={["grid h-7 w-14 place-items-center rounded-full", active && "bg-ink-800"]}><Icon size={20} /></span>
    {label}
    {#if dot}<span class="absolute top-2 left-1/2 ml-3 size-2 rounded-full bg-accent"></span>{/if}
  </button>
{/snippet}

<div data-testid="compact-shell" data-view={mail.view} class="flex h-full flex-col" style:padding-top="var(--safe-top)">
  <!-- What the desktop footer and header say, for screen readers (and the test harness's state line). -->
  <span data-testid="sync-status" data-state={mail.sync.state} class="sr-only">{syncTitle}</span>
  <span data-testid="thread-count" class="sr-only">{count(mail.conversationCount, "conversation")}</span>
  {#if inReader}
    <div class="min-h-0 flex-1"><Reader /></div>
  {:else}
    {#if mail.batching}
      <header data-testid="selection-bar" class="flex h-14 shrink-0 items-center gap-1 border-b border-ink-800 px-1">
        <button data-testid="selection-clear" aria-label="Clear selection" onclick={() => mail.clearChecked()} class="icon-btn size-11"><X size={20} /></button>
        <span data-testid="selection-count" class="flex-1 text-[15px] text-ink-100">{mail.batch.length} selected</span>
        <button data-testid="selection-archive" aria-label="Archive" onclick={() => mail.triage(mail.triageFor("archive"))} class="icon-btn size-11"><Archive size={20} /></button>
        <button data-testid="selection-trash" aria-label="Delete" onclick={() => mail.triage(mail.triageFor("trash"))} class="icon-btn size-11"><Trash2 size={20} /></button>
        <button data-testid="selection-read" aria-label="Toggle read" onclick={() => mail.toggleRead()} class="icon-btn size-11"><MailOpen size={20} /></button>
        <button data-testid="selection-snooze" aria-label="Snooze" onclick={() => mail.startSnooze()} class="icon-btn size-11"><AlarmClock size={20} /></button>
      </header>
    {:else if searching && !inCalendar}
      <header data-testid="inbox" class="flex h-14 shrink-0 items-center gap-1 border-b border-ink-800 px-1">
        <Search size={18} class="ml-3 shrink-0 text-ink-500" />
        <input
          data-testid="search-input"
          bind:this={searchInput}
          value={mail.search}
          oninput={(e) => mail.setSearch(e.currentTarget.value)}
          onkeydown={searchKeys}
          placeholder="Search mail"
          enterkeyhint="search"
          spellcheck="false"
          autocomplete="off"
          class="min-w-0 flex-1 bg-transparent px-2 text-base text-ink-100 outline-none select-text"
        />
        <button data-testid="search-close" aria-label="Close search" onclick={() => mail.endSearch()} class="icon-btn size-11"><X size={20} /></button>
      </header>
    {:else}
      <header data-testid="inbox" class="flex h-14 shrink-0 items-center gap-1 border-b border-ink-800 px-1">
        <button data-testid="drawer-open" aria-label="Mailboxes" onclick={() => (drawer = true)} class="icon-btn size-11"><Menu size={20} /></button>
        <h2 data-testid="mailbox-title" data-mailbox={inCalendar ? "calendar" : mail.mailbox} class="min-w-0 flex-1 truncate px-1 text-lg font-semibold text-ink-100">{title}</h2>
        {#if mail.sync.state !== "idle"}
          <span data-testid="compact-sync" data-state={mail.sync.state} title={syncTitle} class={["grid size-11 place-items-center", mail.sync.state === "error" ? "text-red-300" : "text-ink-500"]}>
            {#if mail.sync.state === "syncing"}<LoaderCircle size={18} class="animate-spin" />{:else}<AlertCircle size={18} />{/if}
          </span>
        {/if}
        {#if !inCalendar}
          <button data-testid="header-search" aria-label="Search" onclick={startSearch} class="icon-btn size-11"><Search size={20} /></button>
        {/if}
      </header>
    {/if}

    {#if !inCalendar && !searching && mail.mailbox === "inbox" && mail.visibleSplits.length > 1}
      <nav data-testid="splits" class="flex shrink-0 gap-1 overflow-x-auto border-b border-ink-800 px-2 py-1.5 [scrollbar-width:none]">
        {#each mail.visibleSplits as i (mail.splits[i].name)}
          {@const s = mail.splits[i]}
          <button
            data-testid="split-tab"
            data-active={i === mail.splitIndex}
            data-unread={s.unread > 0}
            onclick={() => mail.switchSplit(i)}
            class={["shrink-0 rounded-full px-3.5 py-1.5 text-sm", i === mail.splitIndex ? "bg-ink-800 text-ink-100" : "text-ink-500", s.unread > 0 && "font-semibold"]}
          >
            {s.name}
            <span data-testid="split-count" class="ml-1 text-xs tabular-nums">{s.count}</span>
          </button>
        {/each}
      </nav>
    {/if}

    <main data-testid="main" data-layout="compact" class="relative min-h-0 flex-1">
      {#if inCalendar}
        <div class="flex h-full flex-col">
          <DayView onselect={(e) => {
            calendar.selected = e;
            mail.overlay = "event";
          }} />
        </div>
      {:else if special && mail.mailbox === "scheduled"}
        <ScheduledList />
      {:else if special}
        <DraftsList />
      {:else}
        <ThreadList />
      {/if}
      {#if !inCalendar && !mail.batching}
        <button data-testid="compose-fab" aria-label="Compose" onclick={() => mail.compose()} class="absolute right-4 bottom-4 grid size-14 place-items-center rounded-2xl bg-accent text-ink-950 shadow-lg shadow-black/40">
          <SquarePen size={22} />
        </button>
      {/if}
    </main>
  {/if}

  {#if !inReader}
    <nav data-testid="bottom-nav" aria-label="Sections" class="flex shrink-0 border-t border-ink-800 bg-chrome" style:padding-bottom="var(--safe-bottom)">
      {@render navButton("nav-mail", "Mail", MailIcon, !inCalendar && !searching, () => goTo(mail.mailbox), inboxUnread)}
      {@render navButton("nav-calendar", "Calendar", CalendarDays, inCalendar, () => (mail.view = "calendar"))}
      {@render navButton("nav-search", "Search", Search, searching && !inCalendar, startSearch)}
      {@render navButton("nav-settings", "Settings", SettingsIcon, false, () => mail.openSettings())}
    </nav>
  {/if}
</div>

{#if drawer}
  <div data-testid="drawer" role="presentation" onclick={(e) => e.target === e.currentTarget && (drawer = false)} class="fixed inset-0 z-30 bg-black/50">
    <nav aria-label="Mailboxes" class="flex h-full w-72 max-w-[85vw] flex-col bg-ink-900 shadow-2xl" style:padding-top="var(--safe-top)" style:padding-bottom="var(--safe-bottom)">
      <div class="px-5 pt-5 pb-3 text-sm font-semibold text-ink-100">PeetMail</div>
      <div class="min-h-0 flex-1 overflow-y-auto px-2">
        {#each MAILBOXES as m (m.id)}
          {@const active = !inCalendar && !searching && mail.mailbox === m.id}
          <button
            data-testid="drawer-mailbox-{m.id}"
            data-active={active}
            onclick={() => goTo(m.id)}
            class={["flex min-h-12 w-full items-center gap-3 rounded-full px-4 text-left text-[15px]", active ? "bg-ink-800 text-ink-100" : "text-ink-300"]}
          >
            <m.Icon size={19} class={active ? "text-accent" : "text-ink-500"} />
            <span class="flex-1">{m.label}</span>
            {#if m.id === "inbox" && inboxUnread}<span class="size-2 rounded-full bg-accent"></span>{/if}
            {#if m.id === "scheduled" && mail.scheduled.length}<span class="text-xs text-ink-500">{mail.scheduled.length}</span>{/if}
          </button>
        {/each}
        <div class="mx-4 my-2 border-t border-ink-800"></div>
        <button data-testid="drawer-help" onclick={() => { drawer = false; mail.openHelp(); }} class="flex min-h-12 w-full items-center gap-3 rounded-full px-4 text-left text-[15px] text-ink-300">
          <CircleHelp size={19} class="text-ink-500" /> Help
        </button>
      </div>
      <div class="space-y-1 border-t border-ink-800 px-5 py-3 text-xs text-ink-500">
        <p data-testid="drawer-sync" data-state={mail.sync.state} class={mail.sync.state === "error" ? "text-red-300" : ""}>{syncTitle}</p>
        {#if mail.outbox.failed}<p class="text-red-300">{count(mail.outbox.failed, "change")} failed</p>{/if}
        {#each mail.accounts as a (a.id)}<p class="truncate">{a.email}</p>{/each}
      </div>
    </nav>
  </div>
{/if}
