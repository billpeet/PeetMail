<script lang="ts">
  import { mail } from "#lib/mail.svelte.js";
  import { longDate, senderName } from "#lib/format.js";
  import type { MessageRow, Recipient } from "#lib/api.js";
  import MessageBody from "./MessageBody.svelte";
  import Attachments from "./Attachments.svelte";
  import InviteCard from "./InviteCard.svelte";
  import Composer from "./Composer.svelte";
  import { Archive, ArchiveRestore, ArrowLeft, ChevronDown, ChevronUp, Flag, Forward, Inbox, Mail, Maximize2, Reply, ReplyAll, ShieldAlert, ShieldCheck, Trash2 } from "@lucide/svelte";

  /** Shown as the reading pane beside the list (preview layout), not full width. */
  const inPane = $derived(mail.previewing && mail.view === "list");

  const flagged = $derived(mail.current?.isFlagged ?? false);
  /** The reply or forward drafted for this conversation, if any. */
  const threadDraft = $derived(mail.current && mail.threadFor ? mail.draftFor(mail.current.accountId, mail.threadFor) : undefined);
  // In Done, e brings the conversation back to the inbox; in Trash, e and # restore it;
  // in Junk, e and ! mean "not spam".
  const archiveAction = $derived(mail.triageFor("archive"));
  const archiveBack = $derived(archiveAction !== "archive");
  const trashBack = $derived(mail.triageFor("trash") === "moveToInbox");
  const notSpam = $derived(mail.triageFor("spam") === "notSpam");

  /** Label for meeting messages: invitations, updates, and how people answered. */
  function meetingLabel(m: MessageRow): { text: string; tone: string } | null {
    const mine = m.folderRole === "sentitems" || mail.isMe(m.fromAddress);
    switch (m.meetingMessageType) {
      case "meetingRequest": {
        const earlier = mail.thread.some((o) => o.meetingMessageType === "meetingRequest" && o.receivedAt < m.receivedAt);
        return { text: earlier ? "Updated invitation" : "Invitation", tone: "bg-accent/15 text-accent" };
      }
      case "meetingAccepted":
        return { text: mine ? "You accepted" : "Accepted", tone: "bg-emerald-400/15 text-emerald-300" };
      case "meetingTenativelyAccepted":
      case "meetingTentativelyAccepted":
        return { text: mine ? "You said maybe" : "Tentative", tone: "bg-amber-400/15 text-amber-300" };
      case "meetingDeclined":
        return { text: mine ? "You declined" : "Declined", tone: "bg-red-400/15 text-red-300" };
      case "meetingCancelled":
        return { text: "Cancelled", tone: "bg-red-400/15 text-red-300" };
      default:
        return null;
    }
  }

  const images = $derived(mail.imagesFor !== null && mail.imagesFor === mail.threadFor);

  // Newest and unread messages start expanded; the rest collapse to one line.
  // Initialise once per conversation, after its messages have loaded, so background
  // refreshes don't undo what the user expanded.
  let expanded = $state(new Set<number>());
  let initializedFor: string | null = null;

  $effect(() => {
    const msgs = mail.thread;
    const conv = mail.threadFor;
    if (!msgs.length || conv === initializedFor) return;
    initializedFor = conv;
    const open = new Set(msgs.filter((m) => !m.isRead).map((m) => m.id));
    const last = msgs.at(-1);
    if (last) open.add(last.id);
    expanded = open;
  });

  function toggle(id: number) {
    const next = new Set(expanded);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    expanded = next;
  }

  const names = (list: Recipient[]) => list.map((r) => senderName(r.name, r.address)).join(", ");
</script>

{#snippet action(testid: string, label: string, run: () => void, Icon: typeof Archive, extra = "")}
  <button data-testid={testid} title={label} aria-label={label} onclick={run} class="icon-btn {extra}">
    <Icon size={17} />
  </button>
{/snippet}

<div class="flex h-full flex-col">
  <!-- Every keyboard action, also reachable by mouse. Tooltips show the shortcut. -->
  <div data-testid="reader-toolbar" class="flex h-11 shrink-0 items-center gap-1 border-b border-ink-800 px-3">
    {#if inPane}
      {@render action("reader-expand", "Open full width (Enter)", () => mail.open(), Maximize2)}
    {:else}
      {@render action("reader-back", "Back to list (Esc)", () => mail.back(), ArrowLeft)}
    {/if}
    <span class="mx-1 h-5 w-px bg-ink-700"></span>
    {#if archiveBack}
      {@render action("reader-archive", "Move to Inbox (E)", () => mail.act(archiveAction), Inbox)}
    {:else}
      {@render action("reader-archive", "Archive (E)", () => mail.act("archive"), Archive)}
    {/if}
    {#if trashBack}
      {@render action("reader-trash", "Restore to Inbox (#)", () => mail.act("moveToInbox"), ArchiveRestore)}
    {:else}
      {@render action("reader-trash", "Delete (#)", () => mail.act("trash"), Trash2)}
    {/if}
    {#if notSpam}
      {@render action("reader-spam", "Not spam (!)", () => mail.act("notSpam"), ShieldCheck)}
    {:else}
      {@render action("reader-spam", "Spam (!)", () => mail.act("spam"), ShieldAlert)}
    {/if}
    {@render action("reader-unread", "Mark as unread (U)", () => mail.act("markUnread"), Mail)}
    {@render action("reader-flag", flagged ? "Unflag (S)" : "Flag (S)", () => mail.toggleFlag(), Flag, flagged ? "text-amber-400" : "")}
    <span class="mx-1 h-5 w-px bg-ink-700"></span>
    {@render action("reader-reply", "Reply (R)", () => mail.respond("reply"), Reply)}
    {@render action("reader-reply-all", "Reply all (A)", () => mail.respond("replyAll"), ReplyAll)}
    {@render action("reader-forward", "Forward (F)", () => mail.respond("forward"), Forward)}
    <span class="ml-auto text-xs text-ink-500 tabular-nums">{mail.pinned ? "From Recent Opens" : `${mail.selected + 1} of ${mail.threads.length}`}</span>
    {@render action("reader-prev", "Previous conversation (K)", () => mail.move(-1), ChevronUp)}
    {@render action("reader-next", "Next conversation (J)", () => mail.move(1), ChevronDown)}
  </div>

<div data-testid="reader" data-conversation-id={mail.threadFor} class="min-h-0 flex-1 overflow-y-auto">
  <div class="mx-auto max-w-4xl px-8 py-6">
    <h1 class="mb-5 text-xl font-semibold text-ink-100 select-text">{mail.current?.subject || "(no subject)"}</h1>
    {#if mail.invite}<InviteCard invite={mail.invite} />{/if}

    <div class="space-y-2">
      {#each mail.thread as m (m.id)}
        {@const open = expanded.has(m.id)}
        <article data-testid="message" data-message-id={m.id} data-expanded={open} class="overflow-hidden rounded-lg border border-ink-700 bg-ink-900">
          <button data-testid="message-header" class="flex w-full items-baseline gap-3 px-4 py-2.5 text-left text-sm" onclick={() => toggle(m.id)}>
            <span class="shrink-0 font-medium text-ink-100">{senderName(m.fromName, m.fromAddress)}</span>
            {#if meetingLabel(m)}
              {@const label = meetingLabel(m)!}
              <span data-testid="meeting-label" class="shrink-0 rounded px-1.5 text-[11px] font-medium {label.tone}">{label.text}</span>
            {:else if m.folderRole === "sentitems"}
              <span class="shrink-0 rounded bg-ink-800 px-1.5 text-[11px] text-ink-500">Sent</span>
            {/if}
            {#if m.isFlagged}<span class="shrink-0 text-xs text-amber-400" title="Flagged">●</span>{/if}
            {#if open}
              <span class="min-w-0 flex-1 truncate text-xs text-ink-500">
                to {names(m.to)}{m.cc.length ? `, cc ${names(m.cc)}` : ""}
              </span>
            {:else}
              <span class="min-w-0 flex-1 truncate text-ink-500">{m.preview}</span>
            {/if}
            <span class="shrink-0 text-xs text-ink-500">{longDate(m.receivedAt)}</span>
          </button>
          {#if open}
            <div class="px-2 pb-2"><MessageBody messageId={m.id} {images} /></div>
            {#if m.hasAttachments}<Attachments messageId={m.id} />{/if}
          {/if}
        </article>
      {/each}
    </div>
    <!-- A reply or forward drafted for this conversation (here or elsewhere), like Superhuman:
         under the messages; r (or a click) carries on with it. -->
    {#if threadDraft && !(mail.composerInline && mail.composer?.draftId === threadDraft.id)}
      <button
        data-testid="thread-draft"
        onclick={() => mail.openDraft(threadDraft)}
        class="mt-2 flex w-full items-baseline gap-3 rounded-lg border border-dashed border-red-400/50 bg-ink-900 px-4 py-2.5 text-left text-sm hover:border-red-400"
      >
        <span class="shrink-0 font-medium text-red-400">Draft</span>
        <span class="min-w-0 flex-1 truncate text-ink-300">{threadDraft.draft.body.split("\n").find((l) => l.trim()) || "(nothing written yet)"}</span>
        <span class="shrink-0 text-xs text-ink-500"><kbd>R</kbd> to carry on</span>
      </button>
    {/if}
    <!-- Keyed: another composer replacing this one (f during a reply) starts fresh. -->
    {#if mail.composerInline}{#key mail.composer}<Composer inline />{/key}{/if}
  </div>
</div>
</div>
