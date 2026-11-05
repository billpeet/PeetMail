<script lang="ts">
  // Fixed-height virtual list: only the visible rows (plus overscan) are in the DOM,
  // so a 50k-thread inbox costs the same to render as a 50-thread one.
  import { formatSendAt, mail } from "#lib/mail.svelte.js";
  import { listDate, senderName } from "#lib/format.js";
  import type { ThreadRow } from "#lib/api.js";
  import { keyOf } from "#lib/selection.js";

  /** In Snoozed, rows show when they come back instead of when they arrived. */
  const until = (t: ThreadRow): string | null => ("until" in t && typeof t.until === "string" ? t.until : null);
  import { AlarmClock, Archive, Check, Flag, Mail, MailOpen, Paperclip, Trash2 } from "@lucide/svelte";

  // One line per row in the list layout; three (sender + date, subject, preview) in the
  // narrower preview layout, like Outlook.
  const ROW = $derived(mail.previewing ? 76 : 40);
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

  /** Ctrl+click adds a row to the selection (or takes it out), Shift+click a range; a plain click opens it. */
  function onRowClick(e: MouseEvent, index: number) {
    if (e.ctrlKey || e.metaKey) mail.toggleChecked(index);
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

<div bind:this={viewport} bind:clientHeight={height} onscroll={onScroll} data-testid="thread-list" class="h-full overflow-y-auto">
  {#if mail.threads.length === 0}
    <div data-testid="thread-list-empty" class="grid h-full place-items-center text-sm text-ink-500">
      {#if mail.search !== null}No matches.
      {:else if mail.sync.state === "syncing"}Syncing your mail…
      {:else}{({ inbox: "Inbox zero.", done: "Nothing archived yet.", sent: "Nothing sent yet.", snoozed: "Nothing snoozed. Press h on a conversation to snooze it.", scheduled: "", drafts: "", all: "No mail.", trash: "Trash is empty." })[mail.mailbox]}
      {/if}
    </div>
  {:else}
    <div style:height="{mail.threads.length * ROW}px" class="relative">
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
          style:top="{index * ROW}px"
          style:height="{ROW}px"
          class={[
            "group absolute inset-x-0 flex cursor-pointer items-center gap-4 border-l-2 px-5 text-sm",
            selected ? "border-accent bg-ink-800" : checked ? "border-transparent bg-accent/10 hover:bg-accent/15" : "border-transparent hover:bg-ink-900",
          ]}
        >
          {#if mail.previewing}
            <span class="flex min-w-0 flex-1 flex-col gap-0.5 py-2">
              <span class="flex items-baseline gap-2">
                <span class={["min-w-0 flex-1 truncate", unread ? "font-semibold text-ink-100" : "text-ink-300"]}>
                  {#if checked}<span data-testid="row-checked" title="Selected" class="mr-1.5 inline-block align-[-2px] text-accent"><Check size={14} strokeWidth={3} /></span>{/if}{#if mail.accounts.length > 1}<span data-testid="row-account" title={mail.accountOf(t.accountId)?.email} class={["mr-2 inline-block size-2 rounded-full align-middle", mail.accountColor(t.accountId)]}></span>{/if}{@render who(t)}
                  {#if t.messageCount > 1}<span class="ml-1 text-xs font-normal text-ink-500">{t.messageCount}</span>{/if}
                </span>
                {#if t.reminder}<span data-testid="thread-reminder" class="text-xs text-accent" title="Back from a snooze">Reminder</span>{/if}
                {#if t.hasAttachments}<span class="text-ink-500" title="Has attachments"><Paperclip size={13} /></span>{/if}
                {#if t.isFlagged}<span class="text-amber-400" title="Flagged"><Flag size={13} fill="currentColor" /></span>{/if}
                {#if until(t)}
                  <span data-testid="snoozed-until" class="shrink-0 text-xs text-accent group-hover:hidden">{formatSendAt(until(t)!)}</span>
                {:else}
                  <span class={["shrink-0 text-xs tabular-nums group-hover:hidden", unread ? "text-ink-100" : "text-ink-500"]}>{listDate(t.receivedAt)}</span>
                {/if}
              </span>
              <span class={["truncate", unread ? "font-semibold text-ink-100" : "text-ink-300"]}>{#if hasDraft(t)}<span data-testid="thread-draft-badge" class="mr-1.5 font-normal text-red-400">Draft</span>{/if}{t.subject || "(no subject)"}</span>
              <span class="truncate text-xs text-ink-500">{t.preview}</span>
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
          <!-- Quick actions replace the date on hover. -->
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
        </div>
      {/each}
    </div>
  {/if}
</div>
