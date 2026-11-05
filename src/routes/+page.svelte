<script lang="ts">
  import { onMount } from "svelte";
  import { api, onCalendarChanged, onDraftsChanged, onMailChanged, onOutbox, onScheduledChanged, onSettingsChanged, onSyncStatus } from "#lib/api.js";
  import { settingsSync } from "#lib/settingsSync.svelte.js";
  import { calendar } from "#lib/calendar.svelte.js";
  import { keymap } from "#lib/keys.svelte.js";
  import { mail } from "#lib/mail.svelte.js";
  import { updates } from "#lib/updates.svelte.js";
  import { whatsNew } from "#lib/whatsnew.svelte.js";
  import { health } from "#lib/health.svelte.js";
  import HealthBanner from "#lib/components/HealthBanner.svelte";
  import WhatsNew from "#lib/components/WhatsNew.svelte";
  import Help from "#lib/components/Help.svelte";
  import Setup from "#lib/components/Setup.svelte";
  import ThreadList from "#lib/components/ThreadList.svelte";
  import Reader from "#lib/components/Reader.svelte";
  import ShortcutHelp from "#lib/components/ShortcutHelp.svelte";
  import Settings from "#lib/components/Settings.svelte";
  import Composer from "#lib/components/Composer.svelte";
  import Palette from "#lib/components/Palette.svelte";
  import { count } from "#lib/format.js";
  import type { MailView } from "#lib/mail.svelte.js";
  import { Columns2, PanelRight, Rows3, Search, Settings as SettingsIcon, SquarePen, X } from "@lucide/svelte";
  import WeekView from "#lib/components/WeekView.svelte";
  import Rail from "#lib/components/Rail.svelte";
  import EventDetails from "#lib/components/EventDetails.svelte";
  import QuickAdd from "#lib/components/QuickAdd.svelte";
  import EventEditor from "#lib/components/EventEditor.svelte";
  import ResizeHandle from "#lib/components/ResizeHandle.svelte";
  import RightPane from "#lib/components/RightPane.svelte";
  import ScheduledList from "#lib/components/ScheduledList.svelte";
  import DraftsList from "#lib/components/DraftsList.svelte";
  import SnoozePicker from "#lib/components/SnoozePicker.svelte";

  // `base`: signed in, nothing modal open (global keys). `ready`: base, in the mail views
  // (every mail binding hangs off it, so mail keys don't act on a hidden conversation
  // while the calendar is showing).
  // The reader's keys work around a draft under its conversation, unless you're typing in it.
  const base = () => !!mail.account && mail.overlay === null && !mail.composerActive;
  const ready = () => base() && mail.view !== "calendar";
  const inCalendar = () => base() && mail.view === "calendar";
  /** An answerable invite is on screen (reader, or the reading pane). */
  const hasInvite = () =>
    ready() &&
    !mail.batching &&
    (mail.view === "reader" || mail.previewing) &&
    !!mail.invite &&
    !mail.invite.event.isOrganizer &&
    !mail.invite.event.isCancelled &&
    mail.invite.meetingMessageType !== "meetingCancelled";
  /** The list holds conversations (Scheduled and Drafts hold messages, with their own keys). */
  const inThreads = () => ready() && !((mail.mailbox === "scheduled" || mail.mailbox === "drafts") && mail.search === null);
  const inList = () => inThreads() && mail.view === "list";
  const inSnoozed = () => ready() && mail.mailbox === "snoozed" && mail.search === null;
  const inReader = () => ready() && mail.view === "reader";
  const hasThread = () => ready() && !!mail.current;
  /** One conversation is on screen to answer: not while a selection covers the reading pane. */
  const hasOne = () => hasThread() && !mail.batching;
  const inInbox = () => ready() && mail.search === null && mail.mailbox === "inbox";
  /** Focus is in a text field (the search box). */
  const typing = () => {
    const el = document.activeElement as HTMLElement | null;
    return !!el && (el.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(el.tagName));
  };

  /** Names of the mailboxes, for the list header. */
  const MAILBOX_TITLES: Record<MailView, string> = { inbox: "Inbox", done: "Done", sent: "Sent", drafts: "Drafts", snoozed: "Snoozed", scheduled: "Scheduled", all: "All mail", trash: "Trash" };

  let searchInput: HTMLInputElement | undefined = $state();

  // Crash reports are on by default; say so once, with the way to turn them off.
  let privacyNotice = $state(false);
  api.privacy().then((p) => (privacyNotice = (p.configured || p.usageConfigured) && !p.noticeSeen)).catch(() => {});
  // After an update: the changelog lines not seen yet, once, when nothing else is open.
  whatsNew.init().catch(() => {});
  let whatsNewShown = false;
  $effect(() => {
    if (!whatsNewShown && mail.account && whatsNew.seen !== null && mail.overlay === null && !mail.composer && whatsNew.fresh.length) {
      whatsNewShown = true;
      mail.overlay = "whatsnew";
    }
  });

  // Which view an action happened in, for its timing (read by the keymap hook in +layout).
  $effect(() => {
    document.body.dataset.view = mail.overlay ?? mail.view;
  });
  function dismissPrivacyNotice(openSettings: boolean) {
    privacyNotice = false;
    api.dismissPrivacyNotice().catch(() => {});
    if (openSettings) mail.openSettings("privacy");
  }

  function focusSearch() {
    mail.startSearch();
    queueMicrotask(() => searchInput?.focus());
  }

  /** The mouse's back button (XButton1) steps back like Esc. */
  function onmouseup(e: MouseEvent) {
    if (e.button !== 3) return;
    e.preventDefault();
    if (mail.overlay) mail.overlay = null;
    else if (mail.composer) mail.composer = null;
    else if (mail.view === "reader") mail.back();
    else if (mail.search !== null) mail.endSearch();
  }

  function searchKeys(e: KeyboardEvent) {
    // Ctrl+Enter searches the server (a keymap binding) and, like Enter, moves to the results.
    if (e.key === "Enter" && e.ctrlKey) {
      searchInput?.blur();
      return;
    }
    if (e.key === "Enter" || e.key === "ArrowDown") {
      e.preventDefault();
      searchInput?.blur();
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      searchInput?.blur();
      mail.endSearch();
    }
  }

  // A reply stays with its conversation: put away on leaving it, shown again on coming back.
  $effect(() => mail.followScreen());

  onMount(() => {
    mail.init();
    api.outboxStatus().then((s) => mail.setOutbox(s));
    mail.refreshScheduled();
    mail.refreshSnoozed();
    mail.refreshDrafts();
    const unlisten = [
      onMailChanged(() => mail.scheduleRefresh()),
      onSyncStatus((s) => (mail.sync = s)),
      onCalendarChanged(() => calendar.refresh()),
      updates.init(),
      health.init(),
      onScheduledChanged(() => mail.refreshScheduled()),
      onDraftsChanged(() => mail.refreshDrafts()),
      settingsSync.init(),
      onSettingsChanged((keys) => mail.reloadSettings(keys)),
      onOutbox((s) => {
        if (s.failed > mail.outbox.failed && s.lastError) mail.showToast({ text: `Couldn't sync a change: ${s.lastError}`, error: true });
        mail.setOutbox(s);
        // A scheduled send that just went out leaves the list.
        if (mail.scheduled.length) mail.refreshScheduled();
      }),
    ];

    const unregister = keymap.register([
      { id: "list.down", keys: "j", label: "Next conversation", group: "Navigate", when: inThreads, run: () => mail.move(1) },
      { id: "list.up", keys: "k", label: "Previous conversation", group: "Navigate", when: inThreads, run: () => mail.move(-1) },
      { id: "list.down.arrow", keys: "ArrowDown", label: "Next conversation", group: "Navigate", when: inList, run: () => mail.move(1) },
      { id: "list.up.arrow", keys: "ArrowUp", label: "Previous conversation", group: "Navigate", when: inList, run: () => mail.move(-1) },
      { id: "list.open", keys: "Enter", label: "Open conversation", group: "Navigate", when: inList, run: () => mail.open() },
      { id: "reader.back", keys: "Escape", label: "Back to list", group: "Navigate", when: inReader, run: () => mail.back() },
      // Before search.clear: Esc lets go of a selection first, then closes the search.
      { id: "select.clear", keys: "Escape", label: "Clear selection", group: "Select", when: () => inList() && mail.batching, run: () => mail.clearChecked() },
      { id: "search.clear", keys: "Escape", label: "Clear search", group: "Navigate", when: () => inList() && mail.search !== null, run: () => mail.endSearch() },
      { id: "list.top", keys: "g g", label: "Jump to top", group: "Navigate", when: inList, run: () => mail.select(0) },
      { id: "list.bottom", keys: "G", label: "Jump to bottom", group: "Navigate", when: inList, run: () => mail.select(mail.threads.length - 1) },
      { id: "split.next", keys: "Tab", label: "Next split", group: "Navigate", when: inInbox, run: () => mail.cycleSplit(1) },
      { id: "split.prev", keys: "shift+Tab", label: "Previous split", group: "Navigate", when: inInbox, run: () => mail.cycleSplit(-1) },
      { id: "go.inbox", keys: "g i", label: "Go to inbox", group: "Go to", when: base, run: () => mail.switchSplit(0) },
      { id: "go.done", keys: "g e", label: "Done (archived)", group: "Go to", when: base, run: () => mail.goTo("done") },
      { id: "go.sent", keys: "g t", label: "Sent", group: "Go to", when: base, run: () => mail.goTo("sent") },
      { id: "go.all", keys: "g a", label: "All mail", group: "Go to", when: base, run: () => mail.goTo("all") },
      { id: "go.trash", keys: "g #", label: "Trash", group: "Go to", when: base, run: () => mail.goTo("trash") },
      { id: "go.snoozed", keys: "g h", label: "Snoozed", group: "Go to", when: base, run: () => mail.goTo("snoozed") },
      { id: "go.scheduled", keys: "g s", label: "Scheduled messages", group: "Go to", when: base, run: () => mail.goTo("scheduled") },
      { id: "go.drafts", keys: "g d", label: "Drafts", group: "Go to", when: base, run: () => mail.goTo("drafts") },
      { id: "go.calendar", keys: "g c", label: "Go to calendar", group: "Go to", when: base, run: () => (mail.view = "calendar") },
      { id: "section.mail", keys: "1", label: "Mail", group: "Go to", when: base, run: () => mail.showMail() },
      { id: "section.calendar", keys: "2", label: "Calendar", group: "Go to", when: base, run: () => (mail.view = "calendar") },
      { id: "search", keys: "/", label: "Search", group: "Go to", when: base, run: focusSearch },
      { id: "search.server", keys: "ctrl+Enter", label: "Search the server", group: "Go to", when: () => inThreads() && mail.canSearchServer, run: () => mail.searchServer() },
      { id: "calendar.new", keys: "C", label: "New event", group: "Calendar", when: base, run: () => (mail.overlay = "quickadd") },
      { id: "calendar.prev", keys: "ArrowLeft", label: "Previous week", group: "Calendar", when: inCalendar, run: () => calendar.shiftWeek(-1) },
      { id: "calendar.next", keys: "ArrowRight", label: "Next week", group: "Calendar", when: inCalendar, run: () => calendar.shiftWeek(1) },
      { id: "calendar.today", keys: "t", label: "This week", group: "Calendar", when: inCalendar, run: () => calendar.thisWeek() },
      { id: "calendar.back", keys: "Escape", label: "Back to inbox", group: "Calendar", when: inCalendar, run: () => mail.back() },

      // A selection: triage keys act on all of it, and one z undoes it.
      { id: "select.toggle", keys: "x", label: "Select conversation", group: "Select", when: () => inList() && !!mail.current, run: () => mail.toggleChecked() },
      { id: "select.down", keys: "J", label: "Select and move down", group: "Select", when: inList, run: () => mail.extendChecked(1) },
      { id: "select.up", keys: "K", label: "Select and move up", group: "Select", when: inList, run: () => mail.extendChecked(-1) },
      { id: "select.down.arrow", keys: "shift+ArrowDown", label: "Select and move down", group: "Select", when: inList, run: () => mail.extendChecked(1) },
      { id: "select.up.arrow", keys: "shift+ArrowUp", label: "Select and move up", group: "Select", when: inList, run: () => mail.extendChecked(-1) },
      // Ctrl chords reach the keymap from text fields too; there Ctrl+A selects the text.
      { id: "select.all", keys: "ctrl+a", label: "Select all in this view", group: "Select", when: () => inList() && !typing(), run: () => mail.checkAll() },

      // In Done and Trash these bring mail back to the inbox (Mail.triageFor).
      { id: "triage.archive", keys: "e", label: "Archive (in Done: back to Inbox)", group: "Triage", when: hasThread, run: () => mail.triage(mail.triageFor("archive")) },
      { id: "triage.trash", keys: "#", label: "Delete (in Trash: restore)", group: "Triage", when: hasThread, run: () => mail.triage(mail.triageFor("trash")) },
      { id: "triage.read", keys: "u", label: "Toggle read", group: "Triage", when: () => hasThread() && !inSnoozed(), run: () => mail.toggleRead() },
      { id: "snoozed.wake", keys: "u", label: "Back to inbox now (in Snoozed)", group: "Triage", when: () => hasThread() && inSnoozed(), run: () => mail.unsnooze() },
      { id: "triage.flag", keys: "s", label: "Toggle flag", group: "Triage", when: hasThread, run: () => mail.toggleFlag() },
      { id: "triage.snooze", keys: "h", label: "Snooze", group: "Triage", when: hasThread, run: () => mail.startSnooze() },
      { id: "triage.undo", keys: "z", label: "Undo", group: "Triage", when: ready, run: () => mail.undo() },

      { id: "compose.new", keys: "c", label: "Compose", group: "Write", when: base, run: () => mail.compose() },
      { id: "compose.reply", keys: "r", label: "Reply", group: "Write", when: hasOne, run: () => mail.respond("reply") },
      { id: "compose.replyAll", keys: "a", label: "Reply all", group: "Write", when: hasOne, run: () => mail.respond("replyAll") },
      { id: "compose.forward", keys: "f", label: "Forward", group: "Write", when: hasOne, run: () => mail.respond("forward") },

      { id: "layout.toggle", keys: "v", label: "Toggle reading pane", group: "App", when: ready, run: () => mail.toggleLayout() },
      { id: "calendar.pane", keys: "0", label: "Toggle calendar", group: "Calendar", when: base, run: () => mail.toggleCalendarPane() },
      { id: "app.pane", keys: "\\", label: "Toggle right pane", group: "App", when: base, run: () => mail.toggleRightPane() },
      { id: "invite.accept", keys: "y", label: "Accept invite", group: "Calendar", when: hasInvite, run: () => mail.rsvp("accept") },
      { id: "invite.maybe", keys: "m", label: "Maybe (tentative)", group: "Calendar", when: hasInvite, run: () => mail.rsvp("tentative") },
      { id: "invite.decline", keys: "n", label: "Decline invite", group: "Calendar", when: hasInvite, run: () => mail.rsvp("decline") },
      { id: "reader.images", keys: "i", label: "Load images", group: "Read", when: inReader, run: () => (mail.imagesFor = mail.threadFor) },

      { id: "app.palette", keys: "ctrl+k", label: "Command palette", group: "App", when: () => !!mail.account && !mail.composerActive && mail.overlay !== "palette", run: () => (mail.overlay = "palette") },
      { id: "app.quit", keys: "ctrl+q", label: "Quit PeetMail", group: "App", when: () => !!mail.account, run: () => api.quitApp() },
      { id: "app.update.install", keys: "ctrl+shift+u", label: "Restart to update", group: "App", when: () => !!mail.account && updates.ready, run: () => updates.install() },
      { id: "app.help.docs", keys: "F1", label: "Help", group: "App", when: () => !mail.composerActive && mail.overlay !== "docs", run: () => mail.openHelp() },
      { id: "app.health.dismiss", keys: "", label: "Dismiss warnings", group: "App", when: () => health.visible.length > 0, run: () => health.dismiss() },
      { id: "app.feedback", keys: "", label: "Report a problem", group: "App", when: () => !mail.composerActive, run: () => mail.openHelp("report") },
      { id: "app.whatsnew", keys: "", label: "What's new", group: "App", when: () => !!mail.account, run: () => (mail.overlay = "changelog") },
      { id: "app.update.check", keys: "", label: "Check for updates", group: "App", when: () => !!mail.account && !updates.busy, run: () => updates.check() },
      { id: "app.sync", keys: "ctrl+r", label: "Sync now", group: "App", when: base, run: () => mail.syncNow() },
      { id: "app.settings", keys: "ctrl+,", label: "Settings", group: "App", when: base, run: () => mail.openSettings() },
      { id: "app.settingsSync", keys: "", label: "Settings sync", group: "App", when: base, run: () => mail.openSettings("sync") },
      { id: "app.settingsSync.now", keys: "", label: "Sync settings now", group: "App", when: () => base() && settingsSync.on, run: () => settingsSync.syncNow().catch((e) => mail.showToast({ text: String(e), error: true })) },
      { id: "app.help", keys: "?", label: "Keyboard shortcuts", group: "App", when: () => !!mail.account && !mail.composerActive, run: () => (mail.overlay = mail.overlay === "help" ? null : "help") },
      // The event editor handles Esc itself (it asks before discarding changes).
      { id: "app.close", keys: "Escape", label: "Close dialog", group: "App", when: () => mail.overlay !== null && mail.overlay !== "editevent", run: () => {
        mail.overlay = null;
        calendar.selected = null;
      } },
    ]);

    return () => {
      unregister();
      unlisten.forEach((p) => p.then((f) => f()));
    };
  });

  const showPane = $derived(mail.boot?.preferences.showRightPane ?? true);

  // Preview layout: list width (resizable, saved) next to the reading pane.
  const LIST_DEFAULT = 420;
  const LIST_MIN = 300;
  let viewport = $state(typeof window === "undefined" ? 1920 : window.innerWidth);
  const listMax = $derived(Math.max(LIST_MIN, Math.min(800, viewport - 480 - (showPane ? 320 : 0))));
  let listDrag = $state<number | null>(null);
  const listWidth = $derived(
    Math.round(Math.max(LIST_MIN, Math.min(listDrag ?? mail.boot?.preferences.listWidth ?? LIST_DEFAULT, listMax))),
  );

  const syncLabel = $derived(
    mail.sync.state === "syncing" ? "Syncing…" : mail.sync.state === "error" ? "Sync error" : "Up to date",
  );
</script>

<!-- Files dropped outside the composer are ignored; without this WebView2 would open them in place of the app. -->
<svelte:window {onmouseup} ondragover={(e) => e.preventDefault()} ondrop={(e) => e.preventDefault()} bind:innerWidth={viewport} />

<div class="flex h-full flex-col bg-ink-950 text-ink-100">
  {#if !mail.boot}
    <!-- bootstrap is a single local query; nothing to show -->
  {:else if !mail.account}
    <Setup />
  {:else}
    <div class="flex min-h-0 flex-1">
    <Rail />
    {#if mail.view === "calendar"}
      <WeekView />
    {:else}
    <div class="flex min-w-0 flex-1 flex-col">
    <header data-testid="inbox" class="flex h-14 shrink-0 items-center gap-1 border-b border-ink-800 px-4 text-[15px]">
      {#if mail.search === null && mail.mailbox !== "inbox"}
        <h2 data-testid="mailbox-title" data-mailbox={mail.mailbox} class="px-3 py-1.5 font-semibold text-ink-100">{MAILBOX_TITLES[mail.mailbox]}</h2>
        <span data-testid="thread-count" class="ml-3 text-xs text-ink-500">
          {mail.mailbox === "drafts" ? count(mail.drafts.length, "draft") : mail.mailbox === "scheduled" ? count(mail.scheduled.length, "message") : count(mail.conversationCount, "conversation")}
        </span>
      {:else if mail.search === null}
        <nav data-testid="splits" class="flex items-center gap-1">
          {#each mail.visibleSplits as i (mail.splits[i].name)}
            {@const s = mail.splits[i]}
            <button
              data-testid="split-tab"
              data-active={i === mail.splitIndex}
              data-unread={s.unread > 0}
              title={`${count(s.count, "conversation")}, ${s.unread} unread`}
              onclick={() => mail.switchSplit(i)}
              class={[
                "rounded-md px-3 py-1.5",
                i === mail.splitIndex ? "bg-ink-800 text-ink-100" : "text-ink-500 hover:bg-ink-900 hover:text-ink-100",
                // Bold means "has unread", on any tab.
                s.unread > 0 && "font-semibold",
              ]}
            >
              {s.name}
              <span data-testid="split-count" class="ml-1 text-xs tabular-nums">{s.count}</span>
            </button>
          {/each}
        </nav>
        <span data-testid="thread-count" class="ml-3 text-xs text-ink-500">{count(mail.conversationCount, "conversation")}</span>
      {:else}
        <Search size={16} class="ml-2 text-ink-500" />
        <input
          data-testid="search-input"
          bind:this={searchInput}
          value={mail.search}
          oninput={(e) => mail.setSearch(e.currentTarget.value)}
          onkeydown={searchKeys}
          placeholder="from:sam is:unread budget"
          spellcheck="false"
          autocomplete="off"
          class="min-w-0 flex-1 bg-transparent px-1 text-ink-100 outline-none select-text"
        />
        <span data-testid="thread-count" class="text-xs text-ink-500">{count(mail.conversationCount, "conversation")}</span>
        <button data-testid="search-close" title="Close search (Esc)" aria-label="Close search" onclick={() => mail.endSearch()} class="icon-btn"><X size={16} /></button>
      {/if}
      {#if mail.batching}
        <span data-testid="selection-count" class="ml-3 flex items-center gap-2 rounded-md bg-ink-800 px-2 py-0.5 text-xs text-ink-100">
          {mail.batch.length} selected
          <button data-testid="selection-clear" title="Clear selection (Esc)" aria-label="Clear selection" onclick={() => mail.clearChecked()} class="text-ink-500 hover:text-ink-100"><X size={12} /></button>
        </span>
      {/if}
      <span class="ml-auto flex items-center gap-1 pl-4">
        <button data-testid="header-compose" title="Compose (C)" aria-label="Compose" onclick={() => mail.compose()} class="icon-btn"><SquarePen size={17} /></button>
        {#if mail.search === null}
          <button data-testid="header-search" title="Search (/)" aria-label="Search" onclick={focusSearch} class="icon-btn"><Search size={17} /></button>
        {/if}
        <button
          data-testid="toggle-layout"
          title={mail.previewing ? "List only (V)" : "List and reading pane (V)"}
          aria-label="Toggle reading pane"
          onclick={() => mail.toggleLayout()}
          class={["icon-btn", mail.previewing && "text-ink-100"]}
        >{#if mail.previewing}<Rows3 size={17} />{:else}<Columns2 size={17} />{/if}</button>
        <button
          data-testid="toggle-pane"
          title={`${showPane ? "Hide" : "Show"} right pane (\\)`}
          aria-label="Toggle right pane"
          onclick={() => mail.toggleRightPane()}
          class={["icon-btn hidden xl:inline-flex", showPane && "text-ink-100"]}
        ><PanelRight size={17} /></button>
      </span>
    </header>

    <main data-testid="main" data-layout={mail.previewing ? "preview" : "list"} class="flex min-h-0 flex-1">
      {#if mail.mailbox === "scheduled" && mail.search === null}
        <div class="min-w-0 flex-1"><ScheduledList /></div>
      {:else if mail.mailbox === "drafts" && mail.search === null}
        <div class="min-w-0 flex-1"><DraftsList /></div>
      {:else if mail.view === "reader"}
        <div class="min-w-0 flex-1"><Reader /></div>
      {:else if mail.previewing}
        <div data-testid="preview-list" data-width={listWidth} style:width="{listWidth}px" class="relative shrink-0 border-r border-ink-800">
          <ThreadList />
          <ResizeHandle
            edge="right"
            testid="list-resize"
            label="Resize message list"
            value={listWidth}
            min={LIST_MIN}
            max={listMax}
            defaultValue={LIST_DEFAULT}
            onpreview={(w) => (listDrag = w)}
            onchange={(w) => mail.setPreference("listWidth", w)}
          />
        </div>
        <div data-testid="preview-pane" class="min-w-0 flex-1">
          {#if mail.batching}
            <div data-testid="selection-summary" class="grid h-full place-items-center text-sm text-ink-500">
              <div class="space-y-3 text-center">
                <p class="text-base text-ink-100">{count(mail.batch.length, "conversation")} selected</p>
                <p><kbd>e</kbd> archive · <kbd>#</kbd> delete · <kbd>u</kbd> read · <kbd>s</kbd> flag · <kbd>h</kbd> snooze</p>
                <p><kbd>x</kbd> select or deselect · <kbd>Esc</kbd> clear</p>
              </div>
            </div>
          {:else if mail.current}
            <Reader />
          {:else}
            <div class="grid h-full place-items-center text-sm text-ink-500">Nothing selected</div>
          {/if}
        </div>
      {:else}
        <div class="min-w-0 flex-1"><ThreadList /></div>
      {/if}
    </main>
    </div>
    {/if}
    <!-- Hidden on narrow windows (< 1280px) so the list keeps its room. -->
    {#if showPane}<div class="hidden xl:flex"><RightPane /></div>{/if}
    </div>

    <footer class="flex h-8 shrink-0 items-center gap-4 border-t border-ink-800 bg-chrome px-4 text-xs text-ink-500">
      <span data-testid="sync-status" data-state={mail.sync.state} class={mail.sync.state === "error" ? "text-red-300" : ""} title={mail.sync.message ?? ""}>{syncLabel}</span>
      {#if mail.outbox.pending && mail.outboxSlow}<span data-testid="outbox-pending">Sending {count(mail.outbox.pending, "change")}…</span>{/if}
      {#if mail.outbox.failed}<span data-testid="outbox-failed" class="text-red-300" title={mail.outbox.lastError ?? ""}>{mail.outbox.failed} failed</span>{/if}
      {#if mail.scheduled.length}
        <button data-testid="footer-scheduled" onclick={() => mail.goTo("scheduled")} class="hover:text-ink-100" title="Scheduled messages (g s)">
          {count(mail.scheduled.length, "scheduled message")}
        </button>
      {/if}
      {#if updates.ready}
        <button data-testid="update-ready" onclick={() => updates.install()} class="text-accent hover:text-ink-100" title="Restart to update (Ctrl+Shift+U)">
          {updates.status?.version} is ready · restart to update <kbd>Ctrl</kbd> <kbd>Shift</kbd> <kbd>U</kbd>
        </button>
      {/if}
      {#if keymap.pending.length}<span class="text-ink-300">{keymap.pending.join(" ")} …</span>{/if}
      <span class="ml-auto flex items-center gap-3">
        <button data-testid="footer-palette" onclick={() => (mail.overlay = "palette")} class="hover:text-ink-100"><kbd>Ctrl</kbd> <kbd>K</kbd> commands</button>
        <button data-testid="footer-help" onclick={() => (mail.overlay = "help")} class="hover:text-ink-100"><kbd>?</kbd> shortcuts</button>
        <!-- With several accounts, a legend for the coloured dots on mail and events; click
             one to hide or show that account's mail and events. -->
        <span data-testid="footer-accounts" class="flex items-center gap-3 text-ink-500">
          {#if mail.accounts.length > 1}
            {#each mail.accounts as a (a.id)}
              {@const hidden = mail.hiddenIds.includes(a.id)}
              <button
                data-testid="footer-account"
                data-email={a.email}
                data-hidden={hidden}
                aria-pressed={!hidden}
                onclick={() => mail.toggleAccount(a.id)}
                title="{a.provider === 'google' ? 'Gmail' : 'Microsoft'} · click to {hidden ? 'show' : 'hide'} this account's mail and events"
                class={["flex max-w-56 items-center gap-1.5 hover:text-ink-100", hidden && "opacity-50"]}
              >
                <span class={["size-2 shrink-0 rounded-full", hidden ? "border border-ink-500 bg-transparent" : mail.accountColor(a.id)]}></span>
                <span class={["truncate", hidden && "line-through"]}>{a.email}</span>
              </button>
            {/each}
          {:else}
            {mail.account.email}
          {/if}
        </span>
        <button data-testid="footer-settings" title="Settings (Ctrl+,)" aria-label="Settings" onclick={() => mail.openSettings()} class="icon-btn size-6"><SettingsIcon size={14} /></button>
      </span>
    </footer>
  {/if}

  {#if mail.toast}
    <div
      data-testid="toast"
      class={[
        "fixed bottom-10 left-1/2 z-40 -translate-x-1/2 rounded-md border px-4 py-2 text-sm shadow-xl select-text",
        mail.toast.error ? "border-red-900/60 bg-red-950/90 text-red-200" : "border-ink-700 bg-ink-800 text-ink-100",
      ]}
    >
      {mail.toast.text}
      {#if mail.toast.undo}<span class="ml-3 text-ink-500"><kbd>z</kbd> undo</span>{/if}
    </div>
  {/if}

  {#if mail.account && privacyNotice && mail.overlay === null}
    <div data-testid="privacy-notice" role="status" class="fixed bottom-10 left-4 z-30 max-w-sm space-y-2 rounded-lg border border-ink-700 bg-ink-900 p-4 text-sm text-ink-300 shadow-xl">
      <p>PeetMail sends crash reports and usage and performance data so problems get found and fixed. Mail, addresses, names and sign-in details are never included.</p>
      <div class="flex justify-end gap-2">
        <button data-testid="privacy-notice-settings" onclick={() => dismissPrivacyNotice(true)} class="rounded-md px-3 py-1 text-ink-300 hover:text-ink-100">Settings</button>
        <button data-testid="privacy-notice-ok" onclick={() => dismissPrivacyNotice(false)} class="rounded-md bg-accent px-3 py-1 font-medium text-ink-950">Got it</button>
      </div>
    </div>
  {/if}

  {#if mail.overlay === null && !mail.composerActive}<HealthBanner />{/if}

  {#if mail.error}
    <div data-testid="app-error" class="fixed right-4 bottom-10 max-w-md rounded-md border border-red-900/60 bg-red-950/80 px-3 py-2 text-sm text-red-200 select-text">
      {mail.error}
    </div>
  {/if}
</div>

<!-- Replies to the conversation on screen are written inline in the reader instead. -->
{#if mail.composer && !mail.composerInline}{#key mail.composer}<Composer />{/key}{/if}
{#if mail.overlay === "help"}<ShortcutHelp />{/if}
{#if mail.overlay === "docs"}<Help />{/if}
{#if mail.overlay === "whatsnew"}<WhatsNew mode="new" />{/if}
{#if mail.overlay === "changelog"}<WhatsNew mode="all" />{/if}
{#if mail.overlay === "settings"}<Settings />{/if}
{#if mail.overlay === "palette"}<Palette />{/if}
{#if mail.overlay === "event"}<EventDetails />{/if}
{#if mail.overlay === "quickadd"}<QuickAdd />{/if}
{#if mail.overlay === "editevent" && calendar.editing}<EventEditor />{/if}
{#if mail.overlay === "snooze"}<SnoozePicker />{/if}
