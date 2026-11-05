<script lang="ts">
  // Slim left sidebar, like Superhuman's: the mailboxes (each across every account), then
  // the calendar. Icons only; the tooltips name them and their keys.
  import { AlarmClock, CalendarDays, CircleCheck, Clock, Inbox, Mails, PencilLine, Send, ShieldAlert, Trash2 } from "@lucide/svelte";
  import { mail } from "#lib/mail.svelte.js";
  import type { MailView } from "#lib/mail.svelte.js";

  type Item = { id: string; label: string; key: string; Icon: typeof Inbox; go: () => void; active: boolean; dot?: boolean };

  const inMail = $derived(mail.view !== "calendar" && mail.search === null);
  const box = (id: MailView, label: string, key: string, Icon: typeof Inbox, dot = false): Item => ({
    id: `mailbox-${id}`,
    label,
    key,
    Icon,
    go: () => mail.goTo(id),
    active: inMail && mail.mailbox === id,
    dot,
  });
  const inboxUnread = $derived(mail.splits.some((s) => s.unread > 0));

  const mailItems = $derived<Item[]>([
    box("inbox", "Inbox", "g i", Inbox, inboxUnread),
    box("done", "Done", "g e", CircleCheck),
    box("sent", "Sent", "g t", Send),
    box("drafts", "Drafts", "g d", PencilLine),
    box("snoozed", "Snoozed", "g h", AlarmClock),
    box("scheduled", "Scheduled", "g s", Clock),
    box("all", "All mail", "g a", Mails),
    box("junk", "Junk", "g !", ShieldAlert),
    box("trash", "Trash", "g #", Trash2),
  ]);
  const calendarItem = $derived<Item>({ id: "calendar", label: "Calendar", key: "2", Icon: CalendarDays, go: () => (mail.view = "calendar"), active: mail.view === "calendar" });
</script>

{#snippet button(it: Item)}
  <button
    data-testid="rail-{it.id}"
    data-active={it.active}
    aria-current={it.active ? "page" : undefined}
    title="{it.label} ({it.key})"
    aria-label={it.label}
    onclick={it.go}
    class={[
      "relative grid size-9 shrink-0 place-items-center rounded-lg transition-colors",
      it.active ? "bg-ink-800 text-ink-100" : "text-ink-500 hover:bg-ink-900 hover:text-ink-100",
    ]}
  >
    {#if it.active}<span class="absolute top-2 bottom-2 -left-1.5 w-0.5 rounded-full bg-accent"></span>{/if}
    <it.Icon size={18} />
    {#if it.dot}<span data-testid="rail-unread" class="absolute top-1.5 right-1.5 size-1.5 rounded-full bg-accent"></span>{/if}
  </button>
{/snippet}

<nav data-testid="rail" aria-label="Mailboxes" class="flex w-12 shrink-0 flex-col items-center gap-1 overflow-y-auto border-r border-ink-800 bg-chrome py-3">
  {#each mailItems as it (it.id)}{@render button(it)}{/each}
  <span class="my-1 h-px w-6 shrink-0 bg-ink-800"></span>
  {@render button(calendarItem)}
</nav>
