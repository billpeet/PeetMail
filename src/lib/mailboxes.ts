// The mailboxes, in the order the rail (desktop) and the drawer (phone) list them, each
// across every account. The keys are the keymap's `g` sequences.

import { AlarmClock, CircleCheck, Clock, Inbox, Mails, PencilLine, Send, ShieldAlert, Trash2 } from "@lucide/svelte";
import type { MailView } from "./mail.svelte";

export type Mailbox = { id: MailView; label: string; key: string; Icon: typeof Inbox };

export const MAILBOXES: Mailbox[] = [
  { id: "inbox", label: "Inbox", key: "g i", Icon: Inbox },
  { id: "done", label: "Done", key: "g e", Icon: CircleCheck },
  { id: "sent", label: "Sent", key: "g t", Icon: Send },
  { id: "drafts", label: "Drafts", key: "g d", Icon: PencilLine },
  { id: "snoozed", label: "Snoozed", key: "g h", Icon: AlarmClock },
  { id: "scheduled", label: "Scheduled", key: "g s", Icon: Clock },
  { id: "all", label: "All mail", key: "g a", Icon: Mails },
  { id: "junk", label: "Junk", key: "g !", Icon: ShieldAlert },
  { id: "trash", label: "Trash", key: "g #", Icon: Trash2 },
];

/** Names of the mailboxes, for headers. */
export const MAILBOX_TITLES = Object.fromEntries(MAILBOXES.map((m) => [m.id, m.label])) as Record<MailView, string>;
