<script lang="ts">
  // Fixed-height virtual list: only the visible rows (plus overscan) are in the DOM,
  // so a 50k-thread inbox costs the same to render as a 50-thread one.
  import { formatSendAt, mail } from "#lib/mail.svelte.js";
  import { listDate, senderName } from "#lib/format.js";
  import type { ThreadRow } from "#lib/api.js";
  import { keyOf } from "#lib/selection.js";

  /** In Snoozed, rows show when they come back instead of when they arrived. */
  const until = (t: ThreadRow): string | null => ("until" in t && typeof t.until === "string" ? t.until : null);
  import { AlarmClock, Archive, Check, Cloud, Flag, LoaderCircle, Mail, MailOpen, Paperclip, RefreshCw, Trash2 } from "@lucide/svelte";
  import { dragAxis, LONG_PRESS_MS, pullDistance, PULL_TRIGGER_PX, rowSwipe, rowThreshold } from "#lib/gestures.js";
  import { count } from "#lib/format.js";
  import { layout } from "#lib/layout.svelte.js";

  // One line per row in the list layout; three (sender + date, subject, preview) in the
  // narrower preview layout, like Outlook, and on a phone (a little taller, for fingers).
  const stacked = $derived(mail.previewing || layout.compact);
  const ROW = $derived(layout.compact ? 84 : mail.previewing ? 76 : 40);
  const OVERSCAN = 10;

  let viewport: HTMLDivElement | undefined = $state();
  let scrollTop = $state(0);
  let height = $state(0);

  const start = $derived(Math.max(0, Math.floor(scrollTop / ROW) - OVERSCAN));
  const end = $derived(Math.min(mail.threads.length, Math.ceil((scrollTop + height) / ROW) + OVERSCAN));
  const visible = $derived(mail.threads.slice(start, end));

  // Keep the selected row in view without animating; smooth scrolling feels slow under j/k.
  $effect(() => {
    const top = mail.selected * ROW;
    if (!viewport) return;
    if (top < viewport.scrollTop) viewport.scrollTop = top;
    else if (top + ROW > viewport.scrollTop + viewport.clientHeight) viewport.scrollTop = top + ROW - viewport.clientHeight;
  });

  /** A conversation with a reply or forward drafted (here, in Outlook or in Gmail). */
  const hasDraft = (t: ThreadRow) => t.draftId === undefined && !!mail.draftFor(t.accountId, t.conversationId);

  // ---- Touch (phone layout): swipe a row, long-press to select, pull to sync. ----
  type Drag = { index: number; pointer: number; x0: number; y0: number; dx: number; axis: "x" | "y" | null; width: number };
  let drag = $state<Drag | null>(null);
  let pressTimer: ReturnType<typeof setTimeout> | undefined;
  /**
   * A drag or long press just ended on a row: a click the browser sends straight after it
   * isn't a tap. Only straight after: Android often sends none, and a stale flag would eat
   * the next real tap.
   */
  let swallowUntil = 0;
  const swallowClicks = () => (swallowUntil = performance.now() + 400);

  function rowDown(e: PointerEvent, index: number) {
    if (!layout.compact || e.pointerType !== "touch") return;
    drag = { index, pointer: e.pointerId, x0: e.clientX, y0: e.clientY, dx: 0, axis: null, width: (e.currentTarget as HTMLElement).clientWidth };
    clearTimeout(pressTimer);
    // A long press starts a selection (or adds to it), as x does.
    pressTimer = setTimeout(() => {
      if (drag?.index !== index || drag.axis !== null) return;
      drag = null;
      swallowClicks();
      mail.toggleChecked(index);
      navigator.vibrate?.(15);
    }, LONG_PRESS_MS);
  }

  function rowMove(e: PointerEvent) {
    if (!drag || drag.pointer !== e.pointerId) return;
    const dx = e.clientX - drag.x0;
    drag.axis ??= dragAxis(dx, e.clientY - drag.y0);
    if (drag.axis !== null) clearTimeout(pressTimer);
    if (drag.axis === "y") {
      drag = null; // the list scrolls; the browser takes the touch from here
      return;
    }
    // A selection is being made: rows don't swipe, and drafts have nothing to swipe to.
    if (drag.axis === "x" && !mail.batching && mail.threads[drag.index]?.draftId === undefined) {
      (e.currentTarget as HTMLElement).setPointerCapture?.(e.pointerId);
      drag.dx = dx;
    }
  }

  function rowUp(e: PointerEvent) {
    clearTimeout(pressTimer);
    if (!drag || drag.pointer !== e.pointerId) return;
    const { index, dx, axis, width } = drag;
    drag = null;
    if (axis !== "x") return;
    swallowClicks();
    const t = mail.threads[index];
    const action = rowSwipe(dx, width);
    if (!t || !action) return;
    if (action === "archive") mail.actAt(index, mail.triageFor("archive"));
    else {
      mail.selected = index;
      mail.startSnooze(t);
    }
  }

  function rowCancel() {
    clearTimeout(pressTimer);
    drag = null;
  }

  const offset = (index: number) => (drag?.index === index && drag.axis === "x" ? drag.dx : 0);
  const past = $derived(!!drag && Math.abs(drag.dx) >= rowThreshold(drag.width));

  // Pull down at the top of the list to sync, as Ctrl+R does.
  let pullStart: number | null = null;
  let pull = $state(0);
  function pullDown(e: TouchEvent) {
    pullStart = layout.compact && viewport?.scrollTop === 0 ? e.touches[0].clientY : null;
  }
  function pullMove(e: TouchEvent) {
    if (pullStart === null || drag?.axis === "x") return;
    pull = viewport?.scrollTop === 0 ? pullDistance(e.touches[0].clientY - pullStart) : 0;
  }
  function pullUp() {
    if (pull >= PULL_TRIGGER_PX) mail.syncNow();
    pullStart = null;
    pull = 0;
  }

  /** Ctrl+click adds a row to the selection (or takes it out), Shift+click a range; a plain click opens it. */
  function onRowClick(e: MouseEvent, index: number) {
    if (performance.now() < swallowUntil) {
      swallowUntil = 0;
      return;
    }
    // Phone, while selecting: a tap adds the row or takes it out.
    if (layout.compact && mail.batching) mail.toggleChecked(index);
    else if (e.ctrlKey || e.metaKey) mail.toggleChecked(index);
    else if (e.shiftKey) mail.checkRangeTo(index);
    else mail.openAt(index);
  }

  function onScroll() {
    if (!viewport) return;
    scrollTop = viewport.scrollTop;
    if (end >= mail.threads.length - OVERSCAN) mail.loadMore();
  }
</script>

{#snippet who(t: ThreadRow)}
  {#if t.draftId !== undefined}<span data-testid="draft-label" class="text-red-400">Draft</span> {t.draftTo || "(no recipients)"}{:else}{senderName(t.fromName, t.fromAddress)}{/if}
{/snippet}

<!-- Under search results once they've all loaded: search the server for older mail. -->
{#snippet serverSearch()}
  {#if mail.search?.trim() && mail.listComplete}
    <div data-testid="server-search" data-state={mail.canSearchServer ? "offer" : mail.server?.state} class="flex items-center justify-center gap-2 px-5 py-3 text-sm text-ink-500">
      {#if mail.canSearchServer}
        <button data-testid="server-search-run" onclick={() => mail.searchServer()} class="flex items-center gap-2 rounded-md px-3 py-1.5 text-ink-300 hover:bg-ink-900 hover:text-ink-100">
          <Cloud size={15} /> Search the server for older mail <span class="text-ink-500"><kbd>Ctrl</kbd> <kbd>Enter</kbd></span>
        </button>
      {:else if mail.server?.state === "searching"}
        <LoaderCircle size={15} class="animate-spin" /> Searching the server…
      {:else if mail.server}
        <Cloud size={15} /> <span data-testid="server-search-found">{count(mail.server.found, "message")} found on the server</span>
      {/if}
    </div>
  {/if}
{/snippet}

<div bind:this={viewport} bind:clientHeight={height} onscroll={onScroll} ontouchstart={pullDown} ontouchmove={pullMove} ontouchend={pullUp} ontouchcancel={pullUp} role="listbox" aria-label="Conversations" tabindex="-1" data-testid="thread-list" class="relative h-full overflow-y-auto">
  {#if pull > 0}
    <div data-testid="pull-refresh" data-ready={pull >= PULL_TRIGGER_PX} style:height="{pull}px" class="flex items-end justify-center overflow-hidden pb-2">
      <span class={["grid size-8 place-items-center rounded-full bg-ink-800", pull >= PULL_TRIGGER_PX ? "text-accent" : "text-ink-500"]} style:transform="rotate({pull * 4}deg)"><RefreshCw size={16} /></span>
    </div>
  {/if}
  {#if mail.threads.length === 0}
    <div data-testid="thread-list-empty" class="grid h-full place-content-center justify-items-center gap-2 text-sm text-ink-500">
      {#if mail.search !== null}No matches.{@render serverSearch()}
      {:else if mail.sync.state === "syncing"}Syncing your mail…
      {:else}{({ inbox: "Inbox zero.", done: "Nothing archived yet.", sent: "Nothing sent yet.", snoozed: "Nothing snoozed. Press h on a conversation to snooze it.", scheduled: "", drafts: "", all: "No mail.", trash: "Trash is empty.", junk: "No junk." })[mail.mailbox]}
      {/if}
    </div>
  {:else}
    <div style:height="{mail.threads.length * ROW}px" class="relative">
      <!-- Under a row being swiped: what letting go will do. -->
      {#if drag && drag.axis === "x" && drag.dx !== 0}
        <div
          data-testid="swipe-under"
          data-action={drag.dx > 0 ? "archive" : "snooze"}
          data-ready={past}
          style:top="{drag.index * ROW}px"
          style:height="{ROW}px"
          class={["absolute inset-x-0 flex items-center px-6 text-white transition-colors", drag.dx > 0 ? "justify-start" : "justify-end", past ? (drag.dx > 0 ? "bg-emerald-600" : "bg-amber-600") : "bg-ink-800"]}
        >
          {#if drag.dx > 0}<Archive size={22} />{:else}<AlarmClock size={22} />{/if}
        </div>
      {/if}
      {#each visible as t, i (t.accountId + ":" + t.conversationId)}
        {@const index = start + i}
        {@const selected = index === mail.selected}
        {@const unread = t.unreadCount > 0}
        {@const checked = mail.checked.has(keyOf(t))}
        <!-- svelte-ignore a11y_click_events_have_key_events -->
        <div
          role="option"
          data-testid="thread-row"
          data-conversation-id={t.conversationId}
          data-account-id={t.accountId}
          data-unread={unread}
          data-draft={t.draftId !== undefined}
          data-checked={checked}
          tabindex="-1"
          aria-selected={selected}
          onclick={(e) => onRowClick(e, index)}
          onpointerdown={(e) => rowDown(e, index)}
          onpointermove={rowMove}
          onpointerup={rowUp}
          onpointercancel={rowCancel}
          oncontextmenu={(e) => layout.compact && e.preventDefault()}
          style:top="{index * ROW}px"
          style:height="{ROW}px"
          style:transform={offset(index) ? `translateX(${offset(index)}px)` : undefined}
          style:touch-action={layout.compact ? "pan-y" : undefined}
          class={[
            "group absolute inset-x-0 flex cursor-pointer items-center gap-4 border-l-2 text-sm",
            // On a phone the row covers what's under it while it's swiped; it slides back when let go.
            layout.compact ? "px-4" : "px-5",
            layout.compact && !selected && !checked && "bg-ink-950",
            layout.compact && !offset(index) && "transition-transform duration-150",
            selected ? "border-accent bg-ink-800" : checked ? "border-transparent bg-accent/10 hover:bg-accent/15" : "border-transparent hover:bg-ink-900",
          ]}
        >
          {#if stacked}
            <span class={["flex min-w-0 flex-1 flex-col gap-0.5 py-2", layout.compact && "text-[15px]"]}>
              <span class="flex items-baseline gap-2">
                <span class={["min-w-0 flex-1 truncate", unread ? "font-semibold text-ink-100" : "text-ink-300"]}>
                  {#if checked}<span data-testid="row-checked" title="Selected" class="mr-1.5 inline-block align-[-2px] text-accent"><Check size={14} strokeWidth={3} /></span>{/if}{#if mail.accounts.length > 1}<span data-testid="row-account" title={mail.accountOf(t.accountId)?.email} class={["mr-2 inline-block size-2 rounded-full align-middle", mail.accountColor(t.accountId)]}></span>{/if}{@render who(t)}
                  {#if t.messageCount > 1}<span class="ml-1 text-xs font-normal text-ink-500">{t.messageCount}</span>{/if}
                </span>
                {#if t.reminder}<span data-testid="thread-reminder" class="text-xs text-accent" title="Back from a snooze">Reminder</span>{/if}
                {#if t.hasAttachments}<span class="text-ink-500" title="Has attachments"><Paperclip size={13} /></span>{/if}
                {#if t.isFlagged}<span class="text-amber-400" title="Flagged"><Flag size={13} fill="currentColor" /></span>{/if}
                {#if until(t)}
                  <span data-testid="snoozed-until" class={["shrink-0 text-xs text-accent", !layout.compact && "group-hover:hidden"]}>{formatSendAt(until(t)!)}</span>
                {:else}
                  <span class={["shrink-0 text-xs tabular-nums", !layout.compact && "group-hover:hidden", unread ? "text-ink-100" : "text-ink-500"]}>{listDate(t.receivedAt)}</span>
                {/if}
              </span>
              <span class={["truncate", unread ? "font-semibold text-ink-100" : "text-ink-300"]}>{#if hasDraft(t)}<span data-testid="thread-draft-badge" class="mr-1.5 font-normal text-red-400">Draft</span>{/if}{t.subject || "(no subject)"}</span>
              <span class={["truncate text-ink-500", layout.compact ? "text-[13px]" : "text-xs"]}>{t.preview}</span>
            </span>
          {:else}
            <span class={["w-48 shrink-0 truncate", unread ? "font-semibold text-ink-100" : "text-ink-300"]}>
              {#if checked}<span data-testid="row-checked" title="Selected" class="mr-1.5 inline-block align-[-2px] text-accent"><Check size={14} strokeWidth={3} /></span>{/if}{#if mail.accounts.length > 1}<span data-testid="row-account" title={mail.accountOf(t.accountId)?.email} class={["mr-2 inline-block size-2 rounded-full align-middle", mail.accountColor(t.accountId)]}></span>{/if}{@render who(t)}
              {#if t.messageCount > 1}<span class="ml-1 text-xs font-normal text-ink-500">{t.messageCount}</span>{/if}
            </span>
            <span class="min-w-0 flex-1 truncate">
              <span class={unread ? "font-semibold text-ink-100" : "text-ink-300"}>{#if hasDraft(t)}<span data-testid="thread-draft-badge" class="mr-1.5 font-normal text-red-400">Draft</span>{/if}{t.subject || "(no subject)"}</span>
              <span class="ml-2 text-ink-500">{t.preview}</span>
            </span>
            {#if t.reminder}<span data-testid="thread-reminder" class="shrink-0 text-xs text-accent" title="Back from a snooze">Reminder</span>{/if}
            {#if t.hasAttachments}<span class="text-ink-500" title="Has attachments"><Paperclip size={14} /></span>{/if}
            {#if t.isFlagged}<span class="text-amber-400" title="Flagged"><Flag size={14} fill="currentColor" /></span>{/if}
            {#if until(t)}
              <span data-testid="snoozed-until" class="w-40 shrink-0 text-right text-xs text-accent group-hover:hidden">{formatSendAt(until(t)!)}</span>
            {:else}
              <span class={["w-20 shrink-0 text-right text-xs tabular-nums group-hover:hidden", unread ? "text-ink-100" : "text-ink-500"]}>
                {listDate(t.receivedAt)}
              </span>
            {/if}
          {/if}
          <!-- Quick actions replace the date on hover. A phone has no hover (a tap would leave
               them stuck on); it gets swipes instead (Stage 3). -->
          {#if !layout.compact}
          <span data-testid="row-actions" class={["hidden shrink-0 justify-end group-hover:flex", mail.previewing ? "absolute top-1 right-3 rounded-md bg-ink-800" : "w-28"]}>
            {#if t.draftId !== undefined}
              <button data-testid="row-trash" title="Discard draft (#)" class="icon-btn size-7" onclick={(e) => { e.stopPropagation(); mail.actAt(index, "trash"); }}>
                <Trash2 size={15} />
              </button>
            {:else}
            <button data-testid="row-snooze" title="Snooze (H)" class="icon-btn size-7" onclick={(e) => { e.stopPropagation(); mail.selected = index; mail.startSnooze(t); }}>
              <AlarmClock size={15} />
            </button>
            <button data-testid="row-archive" title="Archive (E)" class="icon-btn size-7" onclick={(e) => { e.stopPropagation(); mail.actAt(index, mail.triageFor("archive")); }}>
              <Archive size={15} />
            </button>
            <button data-testid="row-trash" title="Delete (#)" class="icon-btn size-7" onclick={(e) => { e.stopPropagation(); mail.actAt(index, mail.triageFor("trash")); }}>
              <Trash2 size={15} />
            </button>
            <button
              data-testid="row-read"
              title={unread ? "Mark as read (U)" : "Mark as unread (U)"}
              class="icon-btn size-7"
              onclick={(e) => { e.stopPropagation(); mail.actAt(index, unread ? "markRead" : "markUnread"); }}
            >
              {#if unread}<MailOpen size={15} />{:else}<Mail size={15} />{/if}
            </button>
            {/if}
          </span>
          {/if}
        </div>
      {/each}
    </div>
    {@render serverSearch()}
  {/if}
</div>
