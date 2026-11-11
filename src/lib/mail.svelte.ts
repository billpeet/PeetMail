import {
  api,
  settingsSyncApi,
  type Action,
  type ElsewhereAccount,
  type Bootstrap,
  type Signature,
  type DraftDoc,
  type DraftFile,
  type DraftKind,
  type SavedDraft,
  type Folder,
  type Mailbox,
  type MessageRow,
  type OutboxStatus,
  type Preferences,
  type RecentOpen,
  type Invite,
  type Rsvp,
  type SplitInfo,
  type SyncStatus,
  type ThreadRow,
  type QueuedSend,
  type SnoozedThread,
} from "./api";
import { textToHtml } from "./richtext";
import type { HelpPage } from "./docs";
import { prefetchBody } from "./bodies";
import type { Snippet } from "./snippets";
import { hasOwnText, startingSignature, withSignature } from "./signatures";
import { calendar } from "./calendar.svelte";
import { layout } from "./layout.svelte";
import { visibility } from "./accounts.svelte";
import * as selection from "./selection";

const PAGE = 200;
/** Most rows `list_threads` returns for one query. */
const LIST_MAX = 1000;
/** Load the next page when the selection gets this close to the end. */
const LOAD_AHEAD = 50;
const REFRESH_DEBOUNCE_MS = 150;
const PREFETCH_DELAY_MS = 120;
const TOAST_MS = 5000;
/** In the preview layout, a conversation counts as read once it's been shown this long. */
const PREVIEW_DWELL_MS = 1500;

export type View = "list" | "reader" | "calendar";
export type Overlay = null | "help" | "settings" | "palette" | "event" | "quickadd" | "snooze" | "editevent" | "whatsnew" | "changelog" | "docs";
/** What the mail list shows: a mailbox, or messages scheduled to send. */
export type MailView = Mailbox | "scheduled" | "drafts";

/** Actions that take a conversation out of the list it's in. */
const REMOVALS: Action[] = ["archive", "trash", "moveToInbox", "spam", "notSpam"];

/** The pages of Settings, in sidebar order. */
export type SettingsPage = "accounts" | "mail" | "calendar" | "notifications" | "storage" | "updates" | "privacy" | "sync" | "apps";

export type ComposerState = {
  /** The account it's sent from. */
  accountId: number;
  kind: DraftKind;
  /** Message replied to or forwarded. */
  referenceMessageId: number | null;
  /** Its conversation: while that's on screen, the composer sits under it in the reader. */
  conversationId: string | null;
  to: string;
  cc: string;
  bcc: string;
  subject: string;
  /** The text as plain text (kept in step with `html` by the composer). */
  body: string;
  /** The text as the editor's HTML. */
  html: string;
  /** Files attached so far. */
  attachments: DraftFile[];
  /** Forwards: names of the original's attachments taken out. */
  droppedAttachments: string[];
  /** The saved draft it's kept in, once it has been saved. */
  draftId: number | null;
  /** A draft started in Outlook or Gmail. */
  imported?: boolean;
  /** false: don't take focus when it appears (a draft shown again under its conversation). */
  focus?: boolean;
};

/**
 * Whether a composer holds anything worth keeping as a draft: something you wrote or
 * attached. A reply's own To and subject are filled in for you, so they don't count.
 */
export function worthKeeping(c: ComposerState): boolean {
  const typed = (s: string) => s.trim().length > 0;
  // The signature it opened with isn't something you wrote.
  if ((c.html ? hasOwnText(c.html) : typed(c.body)) || c.attachments.length) return true;
  if (c.kind === "new") return typed(c.to) || typed(c.cc) || typed(c.bcc) || typed(c.subject);
  if (c.kind === "forward") return typed(c.to) || typed(c.cc) || typed(c.bcc) || c.droppedAttachments.length > 0;
  return typed(c.bcc);
}

/** A new-message draft as a row of the Inbox list. */
function draftRow(d: SavedDraft): ThreadRow {
  return {
    messageId: -d.id,
    accountId: d.accountId,
    conversationId: `draft:${d.id}`,
    subject: d.draft.subject,
    fromName: null,
    fromAddress: null,
    receivedAt: d.updatedAt,
    preview: d.draft.body.split("\n").find((l) => l.trim()) ?? "",
    messageCount: 1,
    unreadCount: 0,
    isFlagged: false,
    hasAttachments: d.draft.attachments.length > 0,
    sortAt: d.updatedAt,
    reminder: false,
    draftId: d.id,
    draftTo: d.draft.to.replace(/[,;\s]+$/, "") || d.draft.bcc.replace(/[,;\s]+$/, ""),
  };
}

/**
 * The Inbox list with new-message drafts among the conversations, newest first, like
 * Superhuman. Drafts older than the oldest loaded conversation wait until more loads
 * (`complete`: nothing more to load).
 */
export function withDraftRows(rows: ThreadRow[], drafts: SavedDraft[], complete: boolean): ThreadRow[] {
  const plain = rows.filter((r) => r.draftId === undefined);
  const oldest = plain.at(-1)?.sortAt;
  const time = (s: string) => Date.parse(s);
  const extra = drafts
    .filter((d) => d.draft.conversationId === null)
    .map(draftRow)
    .filter((r) => complete || oldest === undefined || time(r.sortAt) >= time(oldest));
  return [...plain, ...extra].sort((a, b) => time(b.sortAt) - time(a.sortAt));
}

/** What each open composer last saved (or opened with), so closing one unchanged doesn't
 * save over a newer version of its draft from Outlook or Gmail. */
const lastSaved = new WeakMap<ComposerState, string>();

const draftDoc = (c: ComposerState): DraftDoc => ({
  kind: c.kind,
  referenceMessageId: c.referenceMessageId,
  conversationId: c.conversationId,
  to: c.to,
  cc: c.cc,
  bcc: c.bcc,
  subject: c.subject,
  body: c.body,
  html: c.html,
  attachments: c.attachments,
  droppedAttachments: c.droppedAttachments,
  imported: c.imported,
});

export type Toast = { text: string; undo?: boolean; error?: boolean };

class MailStore {
  boot = $state<Bootstrap | null>(null);
  /** Signed-in accounts; their inboxes make up the unified inbox. */
  accounts = $derived(this.boot?.accounts.filter((a) => a.signedIn) ?? []);
  /** The first signed-in account: the default for new mail and events. */
  account = $derived(this.accounts[0] ?? null);

  accountOf(id: number) {
    return this.boot?.accounts.find((a) => a.id === id) ?? null;
  }

  /** True for any of your own addresses. */
  /** The signature settings for an account, if it has any. */
  signatureOf(accountId: number): Signature | null {
    const email = this.accountOf(accountId)?.email.toLowerCase();
    return this.signatures.find((s) => s.address === email) ?? null;
  }

  /** The signature a new composer starts with: "" when there's none for this kind of message. */
  startingSignature(accountId: number, kind: DraftKind): string {
    return startingSignature(this.signatureOf(accountId), kind);
  }

  isMe(address: string | null | undefined): boolean {
    const a = address?.toLowerCase();
    return !!a && (this.boot?.accounts.some((x) => x.email.toLowerCase() === a) ?? false);
  }

  /** Accounts toggled off in the footer legend (still signed in). */
  get hiddenIds(): number[] {
    return visibility.among(this.accounts.map((a) => a.id));
  }

  /** Footer legend click: hide or show an account's mail and events. */
  async toggleAccount(id: number) {
    if (!visibility.toggle(id, this.accounts.map((a) => a.id))) {
      this.showToast({ text: "At least one account stays visible" });
      return;
    }
    this.selected = 0;
    this.clearChecked();
    await Promise.all([this.refresh(), calendar.refresh()]);
  }

  /** Dot colour that tells accounts apart (by sign-in order). */
  accountColor(id: number): string {
    const i = Math.max(0, this.boot?.accounts.findIndex((a) => a.id === id) ?? 0);
    return ["bg-sky-400", "bg-amber-400", "bg-emerald-400", "bg-rose-400", "bg-violet-400"][i % 5];
  }
  folder = $state<Folder | null>(null);

  splits = $state.raw<SplitInfo[]>([]);
  splitIndex = $state(0);
  /**
   * Indexes of the tabs to show. With "hide empty splits" on, empty splits are left out,
   * but Inbox and the tab you're on always stay, so the view never jumps from under you.
   */
  visibleSplits = $derived(
    this.splits
      .map((_, i) => i)
      .filter((i) => {
        const s = this.splits[i];
        return !this.boot?.preferences.hideEmptySplits || s.builtin || s.count > 0 || i === this.splitIndex;
      }),
  );
  /** Active search text; when non-null the list shows search results across all mail. */
  search = $state<string | null>(null);
  /**
   * The server search for `query` (Ctrl+Enter, or the row under the results): running, or
   * done with what it found. Its results join the list while the search text is unchanged.
   */
  server = $state<{ query: string; state: "searching" | "done"; found: number } | null>(null);
  /** The server search fetched mail that must leave the cache when the search closes. */
  #serverOpen = false;
  /** Conversations opened from server results whose other messages were fetched already. */
  #fetchedConversations = new Set<string>();
  /** What the list shows (the sidebar, g i / g e / g t / g h / g s / g a / g #). */
  mailbox = $state<MailView>("inbox");

  threads = $state.raw<ThreadRow[]>([]);
  selected = $state(0);
  /**
   * A conversation opened from outside the list (Recent Opens: archived, another split,
   * older than the cache window). While set, it's what the reader shows and acts on.
   */
  pinned = $state.raw<ThreadRow | null>(null);
  current = $derived(this.pinned ?? this.threads[this.selected] ?? null);
  recent = $state.raw<RecentOpen[]>([]);
  /** Conversations selected for acting on together (x, Shift+J/K, Ctrl+A), by `selection.keyOf`. */
  checked = $state.raw<Set<string>>(new Set());

  view = $state<View>("list");
  overlay = $state<Overlay>(null);
  /** The Settings page showing (kept between openings, so Ctrl+, returns to it). */
  settingsPage = $state<SettingsPage>("accounts");
  /** The Help page showing (kept between openings, like Settings). */
  helpPage = $state<HelpPage>("getting-started");
  thread = $state.raw<MessageRow[]>([]);
  /** Conversation that `thread` belongs to; lags `current` while a thread loads. */
  threadFor = $state<string | null>(null);
  /** The meeting invite in the open conversation, if any (for the RSVP card and y/m/n). */
  invite = $state.raw<Invite | null>(null);
  #inviteSeq = 0;
  /** Invite messages whose lookup failed: list refreshes and reopening retry them without toasting again. */
  #inviteFailed = new Set<number>();
  /** Remote images allowed for the open conversation. */
  imagesFor = $state<string | null>(null);

  #composer = $state<ComposerState | null>(null);
  /** The open composer. Closing it, or opening another, keeps what's in it as a draft
   * (or, when there's nothing worth keeping, throws it away with its files). Sending and
   * discarding go round this (see `send`, `discardComposer`). */
  get composer(): ComposerState | null {
    return this.#composer;
  }
  set composer(next: ComposerState | null) {
    const prev = this.#composer;
    this.#composer = next;
    if (prev && prev !== next) this.#putAway(prev, next);
  }
  /** Saved drafts, most recently edited first. */
  drafts = $state.raw<SavedDraft[]>([]);
  /** Draft saves and deletes, one at a time and in order (a new draft's id must be known
   * before the next save of it). */
  #draftOps: Promise<unknown> = Promise.resolve();
  /** The composer being sent: it mustn't be saved as a draft again meanwhile. */
  #sending: ComposerState | null = null;
  /** Files the composer is still handing to Rust: sending waits for them. */
  pendingFiles = $state(0);
  /** Focus is inside the composer (set by the composer). */
  composerFocused = $state(false);
  /** Bumped to put the cursor in the open composer (r on a conversation whose draft is showing). */
  composerFocusTick = $state(0);
  /** Composers that have sat under their conversation: leaving it puts them away. */
  #wasInline = new WeakSet<ComposerState>();
  /** The conversation whose draft was closed while it showed: not shown again until it's left. */
  #dismissed: string | null = null;
  /** Composer snippets (;name). */
  snippets = $state.raw<Snippet[]>([]);
  /** Per-account signatures, by address. */
  signatures = $state.raw<Signature[]>([]);
  /** Snoozed conversations, soonest back first. */
  snoozed = $state.raw<SnoozedThread[]>([]);
  /** The conversations the snooze picker acts on (one, or a selection). */
  snoozeTargets = $state.raw<ThreadRow[]>([]);
  /** Messages scheduled to send later, soonest first. */
  scheduled = $state.raw<QueuedSend[]>([]);
  /** The send that `z` takes back while its toast is up. */
  #lastSend: { id: number; until: number } | null = null;
  toast = $state<Toast | null>(null);
  outbox = $state<OutboxStatus>({ pending: 0, failed: 0, lastError: null });
  /** True once changes have been waiting a while (slow or offline); routine 2s sends stay quiet. */
  outboxSlow = $state(false);
  sync = $state<SyncStatus>({ state: "idle", message: null });
  error = $state<string | null>(null);

  /**
   * Bumped by every list reload. Responses that come back after a newer reload started are
   * dropped, so fast Tab presses can't leave one split's mail showing under another's tab.
   */
  #listSeq = 0;
  #exhausted = $state(false);
  #loadingMore: Promise<void> | null = null;
  #refreshTimer: ReturnType<typeof setTimeout> | undefined;
  #prefetchTimer: ReturnType<typeof setTimeout> | undefined;
  #toastTimer: ReturnType<typeof setTimeout> | undefined;
  #searchTimer: ReturnType<typeof setTimeout> | undefined;
  #slowTimer: ReturnType<typeof setTimeout> | undefined;
  #toastAt = 0;
  /** Bumped when you navigate back; an open() still loading then doesn't take you back in. */
  #navSeq = 0;
  #dwellTimer: ReturnType<typeof setTimeout> | undefined;

  /** Preview layout: list on the left, the selected conversation beside it. A phone has room for one. */
  get previewing(): boolean {
    return !layout.compact && this.boot?.preferences.layout === "preview";
  }

  /**
   * A reply or forward whose conversation is on screen is written inline, under the
   * messages it answers. Anything else (new mail, or the conversation isn't showing)
   * opens as a dialog.
   */
  /**
   * The composer has the keyboard: a dialog always; one under its conversation only while
   * focus is in it, so j/k, e and Esc still work in the reader around a draft.
   */
  get composerActive(): boolean {
    return !!this.composer && (!this.composerInline || this.composerFocused);
  }

  /**
   * Keeps a reply with its conversation (an effect calls this whenever the screen changes):
   * leaving the conversation puts its reply away as a draft, rather than popping it out over
   * everything; coming back shows it under the messages again, without taking focus.
   */
  followScreen() {
    const c = this.#composer;
    const t = this.current;
    const showing = !!t && t.draftId === undefined && this.threadFor === t.conversationId && (this.view === "reader" || (this.view === "list" && this.previewing));
    const here = showing ? `${t!.accountId}:${t!.conversationId}` : null;
    if (this.#dismissed !== here) this.#dismissed = null;
    if (c) {
      if (this.composerInline) this.#wasInline.add(c);
      else if (this.#wasInline.has(c)) {
        this.#composer = null;
        this.composerFocused = false;
        this.#putAway(c, null, { quiet: true });
      }
      return;
    }
    if (!showing || this.#dismissed === here) return;
    const d = this.draftFor(t!.accountId, t!.conversationId);
    if (!d) return;
    const shown: ComposerState = { ...d.draft, accountId: d.accountId, draftId: d.id, focus: false };
    this.#composer = shown;
    lastSaved.set(this.#composer!, JSON.stringify([shown.accountId, draftDoc(shown)]));
  }

  /** Puts the cursor in the open composer. */
  focusComposer() {
    this.composerFocusTick++;
  }

  get composerInline(): boolean {
    const c = this.composer;
    return (
      !!c &&
      // A reply, a forward, or a draft that answers this conversation (one started in Outlook).
      c.conversationId !== null &&
      c.conversationId === this.threadFor &&
      c.conversationId === this.current?.conversationId &&
      (this.view === "reader" || (this.view === "list" && this.previewing))
    );
  }

  setOutbox(s: OutboxStatus) {
    this.outbox = s;
    if (s.pending === 0) {
      clearTimeout(this.#slowTimer);
      this.#slowTimer = undefined;
      this.outboxSlow = false;
    } else if (this.#slowTimer === undefined && !this.outboxSlow) {
      this.#slowTimer = setTimeout(() => {
        this.#slowTimer = undefined;
        this.outboxSlow = this.outbox.pending > 0;
      }, 4000);
    }
  }

  /** Everything in the list is loaded (no more pages). */
  get listComplete(): boolean {
    return this.#exhausted;
  }

  /** The list includes the server search's results (the search text it ran for is unchanged). */
  get #withServer(): boolean {
    return this.search !== null && this.server?.state === "done" && this.server.query === this.search;
  }

  /** "Search the server" is on offer: there's search text and no server search for it yet. */
  get canSearchServer(): boolean {
    return !!this.search?.trim() && this.server?.query !== this.search;
  }

  /** Query and scope behind the current list. */
  get listQuery(): { mailbox: Mailbox | null; query: string; split: number | null } {
    // Search covers all mail.
    if (this.search !== null) return { mailbox: null, query: this.search, split: null };
    // The unified inbox of every signed-in account, by split tab.
    if (this.mailbox === "inbox") return { mailbox: "inbox", query: "", split: this.splitIndex };
    return { mailbox: this.mailbox === "scheduled" || this.mailbox === "drafts" ? null : this.mailbox, query: "", split: null };
  }

  /**
   * Snoozed lists soonest back first, each row with its time; Scheduled holds sends, not
   * conversations. Both load whole, not page by page.
   */
  /** The list shows drafts among conversations: the Inbox's own tab, not a split or search. */
  get #draftRowsHere(): boolean {
    return this.search === null && this.mailbox === "inbox" && this.splitIndex === 0;
  }

  #mergeDrafts(rows: ThreadRow[], complete: boolean): ThreadRow[] {
    if (!this.#draftRowsHere) return rows.filter((r) => r.draftId === undefined);
    return withDraftRows(rows, this.drafts.filter((d) => !this.hiddenIds.includes(d.accountId)), complete);
  }

  /** Conversations in the list (not the drafts shown among them). */
  get conversationCount(): number {
    return this.threads.filter((t) => t.draftId === undefined).length;
  }

  /** The draft that answers a conversation (a reply or forward, from here or elsewhere). */
  draftFor(accountId: number, conversationId: string): SavedDraft | undefined {
    return this.drafts.find((d) => d.accountId === accountId && d.draft.conversationId === conversationId);
  }

  get #special(): "snoozed" | "scheduled" | "drafts" | null {
    return this.search === null && (this.mailbox === "snoozed" || this.mailbox === "scheduled" || this.mailbox === "drafts") ? this.mailbox : null;
  }

  async #rows(count: number): Promise<ThreadRow[]> {
    if (this.#special === "scheduled") {
      await this.refreshScheduled();
      return [];
    }
    if (this.#special === "drafts") {
      await this.refreshDrafts();
      return [];
    }
    if (this.#special === "snoozed") {
      const all = await api.listSnoozed();
      this.snoozed = all;
      return all.filter((t) => !this.hiddenIds.includes(t.accountId));
    }
    const { mailbox, query, split } = this.listQuery;
    // One query returns at most LIST_MAX rows; a list loaded further (Ctrl+A) reloads in chunks.
    const out: ThreadRow[] = [];
    let cursor: { sortAt: string; messageId: number } | null = null;
    while (out.length < count) {
      const n = Math.min(LIST_MAX, count - out.length);
      const rows = await api.listThreads(mailbox, query, split, cursor, n, this.hiddenIds, this.#withServer);
      out.push(...rows);
      const last = rows.at(-1);
      if (rows.length < n || !last) break;
      cursor = { sortAt: last.sortAt, messageId: last.messageId };
    }
    return out;
  }

  /**
   * The view conversations are opened from, for the reader: from Trash and Junk they show
   * their deleted or junk messages; elsewhere those stay hidden.
   */
  get #openedFrom(): Mailbox | null {
    return this.search === null && (this.mailbox === "trash" || this.mailbox === "junk") ? this.mailbox : null;
  }

  /**
   * What `e`, `#` and `!` do here: in Done and Trash `e` and `#` bring mail back to the
   * inbox; in Junk `e` and `!` mean "not spam".
   */
  triageFor(action: "archive" | "trash" | "spam"): Action {
    if (this.search !== null) return action;
    if (this.mailbox === "junk") return action === "trash" ? "trash" : "notSpam";
    if (this.mailbox === "done" && action === "archive") return "moveToInbox";
    if (this.mailbox === "trash" && action !== "spam") return "moveToInbox";
    return action;
  }

  /** Opens Help (the user docs), at `page` if given. */
  openHelp(page?: HelpPage) {
    if (page) this.helpPage = page;
    this.overlay = "docs";
  }

  /** Opens Settings, at `page` if given (else where it was last left). */
  openSettings(page?: SettingsPage) {
    if (page) this.settingsPage = page;
    this.overlay = "settings";
  }

  async init() {
    try {
      this.boot = await api.bootstrap();
      this.refreshElsewhere();
      api.listSnippets().then((s) => (this.snippets = s)).catch(() => {});
      api.listSignatures().then((s) => (this.signatures = s)).catch(() => {});
      calendar.setWeekStartsOn(this.boot.preferences.weekStartsOn);
      await this.loadFolder();
    } catch (e) {
      this.error = String(e);
    }
  }

  async loadFolder() {
    if (!this.account) {
      this.folder = null;
      this.threads = [];
      this.splits = [];
      this.view = "list";
      return;
    }
    const folders = await api.listFolders(this.account.id);
    this.folder = folders.find((f) => f.role === "inbox") ?? null;
    await this.refresh();
  }

  /** Accounts another PC syncs that aren't here (Settings → Accounts offers them). */
  elsewhere = $state<ElsewhereAccount[]>([]);

  async refreshElsewhere() {
    this.elsewhere = await settingsSyncApi.elsewhere().catch(() => []);
  }

  /** Settings another device changed (settings sync): shows their new values. */
  async reloadSettings(keys: string[]) {
    if (!this.boot) return;
    if (keys.some((k) => k.startsWith("account:"))) {
      // An account added on another PC is offered here (once, as it arrives); one signed out
      // everywhere has gone from this PC too.
      const before = new Set(this.elsewhere.map((a) => `${a.provider}:${a.email}`));
      await this.init();
      const arrived = this.elsewhere.filter((a) => !before.has(`${a.provider}:${a.email}`) && keys.includes(`account:${a.provider}:${a.email}`));
      if (arrived.length) {
        const who = arrived.length === 1 ? arrived[0].email : `${arrived.length} accounts`;
        this.showToast({ text: `${who} ${arrived.length === 1 ? "is" : "are"} on your other PC. Sign in here from Settings → Accounts.` });
      }
      return;
    }
    const { preferences } = await api.bootstrap();
    this.boot = { ...this.boot, preferences };
    calendar.setWeekStartsOn(preferences.weekStartsOn);
    if (keys.some((k) => k.startsWith("snippet:"))) this.snippets = await api.listSnippets();
    if (keys.some((k) => k.startsWith("signature:"))) this.signatures = await api.listSignatures();
    if (keys.some((k) => k.startsWith("split:"))) await this.refresh();
  }

  /** Called on every `mail-changed` event; coalesces bursts during initial sync. */
  scheduleRefresh() {
    clearTimeout(this.#refreshTimer);
    this.#refreshTimer = setTimeout(() => {
      if (this.account) this.refresh();
      else this.loadFolder();
      this.refreshSnoozed();
    }, REFRESH_DEBOUNCE_MS);
  }

  async setPreference<K extends keyof Preferences>(key: K, value: Preferences[K]) {
    if (!this.boot) return;
    const preferences = { ...this.boot.preferences, [key]: value };
    this.boot = { ...this.boot, preferences };
    if (key === "weekStartsOn") calendar.setWeekStartsOn(value as number);
    await api.setPreferences(preferences);
  }

  async setHideEmptySplits(hide: boolean) {
    await this.setPreference("hideEmptySplits", hide);
  }

  toggleRightPane() {
    this.setPreference("showRightPane", !(this.boot?.preferences.showRightPane ?? true));
  }

  /** `0`: show the calendar in the right pane, or go back to Recent Opens if it's showing. */
  async toggleCalendarPane() {
    const p = this.boot?.preferences;
    if (!p) return;
    if (!p.showRightPane || p.rightPaneTab !== "calendar") {
      await this.setPreference("showRightPane", true);
      await this.setPreference("rightPaneTab", "calendar");
    } else {
      await this.setPreference("rightPaneTab", "recent");
    }
  }

  /** A reload in flight: a page loaded meanwhile would land on a list it's about to replace. */
  #refreshing: Promise<unknown> = Promise.resolve();

  /** Reloads everything currently loaded, keeping the selection on the same conversation. */
  refresh(): Promise<void> {
    const run = this.#reload();
    this.#refreshing = run.catch(() => {});
    return run;
  }

  async #reload() {
    if (!this.account) return;
    const seq = ++this.#listSeq;
    const folderForSplits = null; // split tabs count the unified inbox
    const count = Math.max(PAGE, this.threads.length);
    let [splits, rows, recent] = await Promise.all([
      api.listSplits(folderForSplits, this.hiddenIds),
      this.#rows(count),
      api.recentOpens(),
    ]);
    if (seq !== this.#listSeq) return;
    // The selection as it is now, not when the refresh started: j/k pressed while it was in
    // flight (a sync landing mid-navigation) must not be undone.
    const keep = this.current?.conversationId;
    this.recent = recent;
    this.splits = splits;
    if (this.splitIndex >= splits.length) this.splitIndex = 0;
    this.#exhausted = this.#special !== null || rows.length < count;
    rows = this.#mergeDrafts(rows, this.#exhausted);
    this.threads = rows;
    this.checked = selection.prune(this.checked, rows);
    const idx = keep ? rows.findIndex((t) => t.conversationId === keep) : -1;
    this.selected = idx >= 0 ? idx : Math.min(this.selected, Math.max(0, rows.length - 1));
    this.#schedulePrefetch();
    const cur = this.current;
    if (this.view === "reader" || this.previewing) {
      if (!cur) {
        this.view = "list";
        this.thread = [];
        this.threadFor = null;
        this.invite = null;
      } else {
        const thread = await api.getThread(cur.accountId, cur.conversationId, this.#openedFrom);
        if (seq !== this.#listSeq) return;
        this.thread = thread;
        this.threadFor = cur.conversationId;
        this.#loadInvite();
      }
    }
  }

  /** Loads the next page (or waits for the one already loading). */
  loadMore(): Promise<void> {
    if (this.#loadingMore) return this.#loadingMore;
    if (!this.account || this.#exhausted) return Promise.resolve();
    const run = (async () => {
      // After a reload already under way, so the page follows the list it produces.
      await this.#refreshing;
      const last = this.threads.findLast((t) => t.draftId === undefined);
      if (this.#exhausted || !last) return;
      const seq = this.#listSeq;
      const { mailbox, query, split } = this.listQuery;
      const rows = await api.listThreads(mailbox, query, split, { sortAt: last.sortAt, messageId: last.messageId }, PAGE, this.hiddenIds, this.#withServer);
      if (seq !== this.#listSeq) return; // the list was reloaded meanwhile
      this.#exhausted = rows.length < PAGE;
      this.threads = this.#mergeDrafts([...this.threads, ...rows], this.#exhausted);
    })().finally(() => (this.#loadingMore = null));
    this.#loadingMore = run;
    return run;
  }

  // ---------- selecting several conversations ----------

  /** The selected conversations, in list order. Only the list acts on them, not the reader. */
  get batch(): ThreadRow[] {
    return this.view === "list" ? selection.pick(this.checked, this.threads) : [];
  }

  /** e, #, u, s and h act on the selection rather than the cursor's conversation. */
  get batching(): boolean {
    return this.batch.length > 0;
  }

  clearChecked() {
    if (this.checked.size) this.checked = new Set();
  }

  /** x: selects the cursor's conversation, or takes it out of the selection. */
  toggleChecked(index = this.selected) {
    const t = this.threads[index];
    if (!t) return;
    if (t.draftId !== undefined) return this.showToast({ text: "That's a draft: Enter opens it, # discards it" });
    this.checked = selection.toggle(this.checked, t);
  }

  /** Shift+J / Shift+K: selects the cursor's conversation and the next (previous) one, moving onto it. */
  extendChecked(delta: 1 | -1) {
    if (!this.threads.length) return;
    const from = this.selected;
    const to = Math.max(0, Math.min(from + delta, this.threads.length - 1));
    this.checked = selection.addRange(this.checked, this.threads, from, to);
    this.move(to - from);
  }

  /** Shift+click: selects everything from the cursor to `index`, moving the cursor there. */
  checkRangeTo(index: number) {
    this.checked = selection.addRange(this.checked, this.threads, this.selected, index);
    this.select(index);
  }

  /** Ctrl+A: every conversation in the view, loading the rest of the list first. */
  async checkAll() {
    // Background reloads (a sync, the last action's own) may land meanwhile; only leaving
    // the view stops it.
    const view = this.#viewKey;
    for (let i = 0; i < 10_000 && !this.#exhausted && this.threads.length && view === this.#viewKey; i++) {
      await this.loadMore();
    }
    await this.#refreshing;
    if (view !== this.#viewKey) return;
    this.checked = selection.all(this.threads);
  }

  /** What the list shows; a change means another view. */
  get #viewKey(): string {
    return JSON.stringify([this.mailbox, this.splitIndex, this.search, this.hiddenIds]);
  }

  /** Acts on every selected conversation as one undo step. */
  async #actOnBatch(rows: ThreadRow[], action: Action) {
    try {
      const out = await api.threadsAction(rows.map(ref), action);
      const removal = REMOVALS.includes(action);
      // Those leave the list; read and flag keep the selection, so u then e works.
      if (removal) this.checked = new Set();
      this.showToast(out.label ? { text: out.label, undo: true } : { text: "Nothing to change" });
      await this.refresh();
      if (removal) await this.#afterRemoval();
    } catch (e) {
      this.showToast({ text: String(e), error: true });
    }
  }

  /** e and # from the keyboard: the selection if there is one, else the current conversation. */
  async triage(action: Action) {
    const rows = this.batch;
    if (rows.length) return this.#actOnBatch(rows, action);
    return this.act(action);
  }

  select(index: number) {
    this.pinned = null;
    if (!this.threads.length) return;
    this.selected = Math.max(0, Math.min(index, this.threads.length - 1));
    if (this.selected >= this.threads.length - LOAD_AHEAD) this.loadMore();
    this.#schedulePrefetch();
    if (this.view === "reader") this.open();
    else if (this.previewing) this.preview();
  }

  move(delta: number) {
    this.select(this.selected + delta);
  }

  async open() {
    const t = this.current;
    if (!t) return;
    this.clearChecked(); // the reader acts on the one conversation it shows
    if (t.draftId !== undefined) return this.#openDraftRow(t.draftId);
    const nav = this.#navSeq;
    // Load the thread first (a local query) so the reader never flashes empty or stale.
    const rows = await api.getThread(t.accountId, t.conversationId, this.#openedFrom);
    if (this.current?.conversationId !== t.conversationId) return;
    // Esc (back to the list) while it loaded wins: don't reopen the reader behind it.
    if (nav !== this.#navSeq) return;
    this.thread = rows;
    this.threadFor = t.conversationId;
    this.#loadInvite();
    this.view = "reader";
    this.#fetchRestOfConversation(t);
    api.recordOpen(t.accountId, t.conversationId).then(async () => (this.recent = await api.recentOpens()));
    // Opening a conversation reads it, like every mail client; not worth an undo entry.
    if (t.unreadCount > 0) this.act("markRead", { undoable: false, quiet: true });
  }

  /** Mouse: click a row to open it (in the preview layout, to preview it). */
  async openAt(index: number) {
    this.pinned = null;
    this.clearChecked(); // a plain click shows that one conversation
    this.selected = Math.max(0, Math.min(index, this.threads.length - 1));
    if (this.previewing && this.view === "list") await this.preview();
    else await this.open();
  }

  /**
   * Shows the selected conversation in the reading pane. It's only marked read (and added
   * to Recent Opens) after PREVIEW_DWELL_MS, so skimming past with j doesn't read everything.
   */
  async preview() {
    clearTimeout(this.#dwellTimer);
    const t = this.current;
    if (!t || t.draftId !== undefined) {
      this.thread = [];
      this.threadFor = null;
      this.invite = null;
      return;
    }
    const rows = await api.getThread(t.accountId, t.conversationId, this.#openedFrom);
    if (this.current?.conversationId !== t.conversationId) return;
    this.thread = rows;
    this.threadFor = t.conversationId;
    this.#loadInvite();
    this.#fetchRestOfConversation(t);
    this.#dwellTimer = setTimeout(() => {
      // A selection covers the reading pane: the conversation wasn't on screen.
      if (this.current?.conversationId !== t.conversationId || !this.previewing || this.batching) return;
      api.recordOpen(t.accountId, t.conversationId).then(async () => (this.recent = await api.recentOpens()));
      if (t.unreadCount > 0) this.act("markRead", { undoable: false, quiet: true });
    }, PREVIEW_DWELL_MS);
  }

  async toggleLayout() {
    const next = this.previewing ? "list" : "preview";
    await this.setPreference("layout", next);
    this.view = "list";
    this.pinned = null;
    if (next === "preview") await this.preview();
    else clearTimeout(this.#dwellTimer);
  }

  /** Opens a Recent Opens entry: selects it in the list if it's there, else pins it. */
  async openRecent(r: RecentOpen | ThreadRow) {
    this.search = null;
    this.#closeServerSearch();
    const index = this.threads.findIndex((t) => t.conversationId === r.conversationId && t.accountId === r.accountId);
    if (index >= 0) return this.openAt(index);
    this.pinned = r;
    await this.open();
  }

  /** `1` / the rail: back to mail from wherever you are (the calendar, the reader stays). */
  showMail() {
    if (this.view === "calendar") this.view = "list";
  }

  back() {
    this.#navSeq++;
    this.view = "list";
    this.pinned = null;
    // In the list layout the invite is no longer on screen. An invite lookup still in
    // flight (e.g. from the refresh after an RSVP) mustn't bring it back afterwards.
    if (!this.previewing) {
      this.#inviteSeq++;
      this.invite = null;
      calendar.clearContext();
    }
  }

  // ---------- splits and search ----------

  /** Shows a mailbox (Inbox opens on the tab you were on). */
  async goTo(mailbox: MailView) {
    this.clearChecked();
    this.search = null;
    this.#closeServerSearch();
    this.pinned = null;
    if (this.mailbox !== mailbox) {
      // Drop the old list at once: a key pressed before the new one loads (e, #) must not
      // act on a conversation from the mailbox you just left.
      this.threads = [];
      this.selected = 0;
    }
    this.mailbox = mailbox;
    this.view = "list";
    await this.refresh();
    if (this.previewing) await this.preview();
  }

  async switchSplit(index: number) {
    if (!this.splits.length) return;
    this.search = null;
    this.clearChecked();
    this.#closeServerSearch();
    this.mailbox = "inbox";
    this.splitIndex = (index + this.splits.length) % this.splits.length;
    this.selected = 0;
    this.view = "list";
    await this.refresh();
  }

  /** Next/previous visible tab (Tab / Shift+Tab). */
  async cycleSplit(step: 1 | -1) {
    const vis = this.visibleSplits;
    if (!vis.length) return;
    const pos = vis.indexOf(this.splitIndex);
    const next = vis[(pos + step + vis.length) % vis.length];
    await this.switchSplit(next);
  }


  startSearch() {
    this.search = this.search ?? "";
    this.view = "list";
  }

  setSearch(text: string) {
    this.search = text;
    this.clearChecked();
    // Other words: back to local results until the server is searched for them.
    if (this.server && this.server.query !== text) this.server = null;
    clearTimeout(this.#searchTimer);
    this.#searchTimer = setTimeout(() => {
      this.selected = 0;
      this.refresh();
    }, 120);
  }

  async endSearch() {
    if (this.search === null) return;
    this.search = null;
    this.clearChecked();
    this.#closeServerSearch();
    this.selected = 0;
    await this.refresh();
  }

  /**
   * Searches the server for the search text (mail outside the cache window). The results
   * join the list, newest first among the local matches.
   */
  async searchServer() {
    const query = this.search;
    if (query === null || !query.trim() || this.server?.query === query) return;
    this.server = { query, state: "searching", found: 0 };
    this.#serverOpen = true;
    this.#fetchedConversations.clear();
    try {
      const out = await api.searchServer(query, this.hiddenIds);
      if (this.server?.query !== query || this.search !== query) return;
      this.server = { query, state: "done", found: out.found };
      if (out.failed.length) this.showToast({ text: `Couldn't search ${out.failed.join(", ")}`, error: true });
      await this.refresh();
    } catch (e) {
      if (this.server?.query === query) this.server = null;
      this.showToast({ text: String(e), error: true });
    }
  }

  /** The search closed: what the server search fetched leaves the cache. */
  #closeServerSearch() {
    this.server = null;
    this.#fetchedConversations.clear();
    if (!this.#serverOpen) return;
    this.#serverOpen = false;
    api.clearServerSearch().catch(() => {});
  }

  /**
   * A conversation opened from server results: its other messages may be outside the cache
   * window too, so fetch them (once) and show them when they come.
   */
  async #fetchRestOfConversation(t: ThreadRow) {
    const key = `${t.accountId}:${t.conversationId}`;
    if (!this.#withServer || this.#fetchedConversations.has(key)) return;
    this.#fetchedConversations.add(key);
    try {
      if (!(await api.fetchSearchConversation(t.accountId, t.conversationId))) return;
      if (this.threadFor !== t.conversationId || this.current?.accountId !== t.accountId) return;
      this.thread = await api.getThread(t.accountId, t.conversationId, this.#openedFrom);
    } catch {
      // Offline or the server said no: the messages found still show; try again next open.
      this.#fetchedConversations.delete(key);
    }
  }

  // ---------- triage ----------

  async act(action: Action, { undoable = true, quiet = false } = {}) {
    const t = this.current;
    if (!t) return;
    if (t.draftId !== undefined) {
      if (action === "trash") await this.deleteDraft(t.draftId);
      else this.showToast({ text: "That's a draft: Enter opens it, # discards it" });
      return;
    }
    try {
      const out = await api.threadAction(t.accountId, t.conversationId, action, undoable);
      if (out.label && !quiet) this.showToast({ text: out.label, undo: undoable });
      // mail-changed triggers the refresh; do it now too so the list never lags a keypress.
      await this.refresh();
      if (REMOVALS.includes(action)) await this.#afterRemoval();
    } catch (e) {
      this.showToast({ text: String(e), error: true });
    }
  }

  /**
   * The current conversation just left the list (archived, deleted, snoozed). In the
   * reader, move straight on to the next one; a pinned conversation (opened from Recent
   * Opens or Snoozed) returns to the list instead.
   */
  async #afterRemoval() {
    if (this.view === "reader") {
      if (this.pinned) this.back();
      else if (this.current) await this.open();
    } else if (this.previewing) {
      await this.preview(); // the next conversation slid into place; restart its dwell timer
    }
  }

  // ---------- snooze ----------

  /** Opens the snooze picker for `t` (default: the selection, else the current conversation). */
  startSnooze(t: ThreadRow | null = null) {
    const targets = t ? [t] : this.batching ? this.batch : this.current ? [this.current] : [];
    if (!targets.length) return;
    if (targets.length === 1 && targets[0].draftId !== undefined) return this.showToast({ text: "That's a draft: Enter opens it, # discards it" });
    this.snoozeTargets = targets;
    this.overlay = "snooze";
  }

  async snooze(until: Date) {
    const targets = this.snoozeTargets;
    if (!targets.length) return;
    const cur = this.current;
    const wasCurrent = !!cur && targets.some((t) => t.conversationId === cur.conversationId && t.accountId === cur.accountId);
    // Close at once, so keys typed straight after (g h, j) aren't swallowed by the dialog.
    if (this.overlay === "snooze") this.overlay = null;
    this.snoozeTargets = [];
    try {
      const batch = targets.length > 1;
      const out = batch
        ? await api.snoozeThreads(targets.map(ref), until.toISOString())
        : await api.snoozeThread(targets[0].accountId, targets[0].conversationId, until.toISOString());
      if (batch) this.clearChecked();
      const what = out.changed > 1 ? ` ${out.changed} conversations` : "";
      if (out.label) this.showToast({ text: `Snoozed${what} until ${formatSendAt(until.toISOString())}`, undo: true });
      await this.refresh();
      this.refreshSnoozed();
      if (wasCurrent) await this.#afterRemoval();
    } catch (e) {
      this.showToast({ text: String(e), error: true });
    }
  }

  /** Wakes snoozed conversations now (u in Snoozed): the selection, else the current one. */
  async unsnooze(t: ThreadRow | null = null) {
    const targets = t ? [t] : this.batching ? this.batch : this.current ? [this.current] : [];
    if (!targets.length) return;
    try {
      for (const x of targets) await api.unsnoozeThread(x.accountId, x.conversationId);
      this.clearChecked();
      this.showToast({ text: targets.length > 1 ? `${targets.length} conversations back in your inbox` : "Back in your inbox" });
      await this.refresh();
      this.refreshSnoozed();
      await this.#afterRemoval();
    } catch (e) {
      this.showToast({ text: String(e), error: true });
    }
  }

  async refreshSnoozed() {
    try {
      this.snoozed = await api.listSnoozed();
    } catch {}
  }

  /** Mouse: run an action on a row without opening it (hover buttons). */
  async actAt(index: number, action: Action) {
    this.selected = index;
    await this.act(action);
  }

  /** Finds the newest invite in the loaded thread and fetches its event. */
  async #loadInvite() {
    const seq = ++this.#inviteSeq;
    // The newest actual request (invite or update); responses and cancellations don't get
    // RSVP buttons. Fall back to any meeting message if its kind isn't known yet.
    const newestFirst = [...this.thread].reverse();
    // Gmail doesn't mark invitations; the backend checks the newest received message's
    // text/calendar part (cached after the first look) and answers null for ordinary mail.
    const google = this.accountOf(this.current?.accountId ?? -1)?.provider === "google";
    const msg =
      newestFirst.find((m) => m.meetingMessageType === "meetingRequest") ??
      newestFirst.find((m) => m.messageType === "eventMessage" && !m.meetingMessageType) ??
      (google ? newestFirst.find((m) => m.folderRole !== "sentitems") : undefined);
    if (!msg) {
      this.invite = null;
      calendar.clearContext();
      return;
    }
    try {
      const invite = await api.inviteEvent(msg.id);
      if (seq !== this.#inviteSeq) return;
      this.#inviteFailed.delete(msg.id);
      this.invite = invite;
      // Show the meeting in the calendar sidebar, among everything else that day.
      // A series shows its next meeting: its own start can be months back.
      if (invite) calendar.showInContext(invite.next ?? invite.event);
      else calendar.clearContext();
    } catch (e) {
      if (seq === this.#inviteSeq) {
        this.invite = null;
        calendar.clearContext();
        // Gmail mail is only a guess until its calendar part is read: no toast for what may be ordinary mail.
        const knownInvite = !!msg.meetingMessageType || msg.messageType === "eventMessage";
        if (knownInvite && !this.#inviteFailed.has(msg.id)) this.showToast({ text: `Couldn't load the meeting details: ${e}`, error: true });
        this.#inviteFailed.add(msg.id);
      }
    }
  }

  async rsvp(response: Rsvp) {
    const inv = this.invite;
    if (!inv) return;
    const status = { accept: "accepted", tentative: "tentativelyAccepted", decline: "declined" }[response];
    // An invite fetch still in flight (e.g. from the refresh after opening) would bring
    // back the server's old answer; it must not overwrite this one.
    this.#inviteSeq++;
    try {
      await api.respondToEvent(inv.event.accountId, inv.event.remoteId, response);
      this.invite = { ...inv, event: { ...inv.event, response: status } };
      calendar.focus = this.invite.event;
      this.showToast({ text: { accept: "Accepted", tentative: "Tentatively accepted", decline: "Declined" }[response] });
    } catch (e) {
      this.showToast({ text: String(e), error: true });
    }
  }

  async toggleRead() {
    if (this.batching) return this.triage(selection.readActionFor(this.batch));
    const t = this.current;
    if (t) await this.act(t.unreadCount > 0 ? "markRead" : "markUnread");
  }

  async toggleFlag() {
    if (this.batching) return this.triage(selection.flagActionFor(this.batch));
    const t = this.current;
    if (t) await this.act(t.isFlagged ? "unflag" : "flag");
  }

  async undo() {
    const s = this.#lastSend;
    if (s && Date.now() < s.until) {
      this.#lastSend = null;
      const ok = await this.unsend(s.id).catch(() => false);
      this.showToast(ok ? { text: "Send undone" } : { text: "Too late: it's already been sent", error: true });
      return;
    }
    try {
      const label = await api.undo();
      this.showToast({ text: label ? `Undid: ${label.toLowerCase()}` : "Nothing to undo" });
      await this.refresh();
      this.refreshSnoozed();
    } catch (e) {
      this.showToast({ text: String(e), error: true });
    }
  }

  showToast(t: Toast) {
    // A routine toast ("Created") mustn't wipe an error that has only just appeared.
    if (!t.error && this.toast?.error && Date.now() - this.#toastAt < 2000) return;
    this.toast = t;
    this.#toastAt = Date.now();
    clearTimeout(this.#toastTimer);
    this.#toastTimer = setTimeout(() => (this.toast = null), TOAST_MS);
  }

  // ---------- drafts ----------

  #queueDraftOp<T>(op: () => Promise<T>): Promise<T> {
    const run = this.#draftOps.then(op, op);
    this.#draftOps = run.catch(() => {});
    return run;
  }

  /** Saves a composer as a draft (or removes its draft once it's empty again). */
  #saveDraft(c: ComposerState): Promise<void> {
    return this.#queueDraftOp(async () => {
      if (this.#sending === c) return;
      if (!worthKeeping(c)) {
        if (c.draftId !== null) await api.deleteDraft(c.draftId);
        c.draftId = null;
        lastSaved.delete(c);
        return;
      }
      const doc = $state.snapshot(draftDoc(c)) as DraftDoc;
      const json = JSON.stringify([c.accountId, doc]);
      if (c.draftId !== null && lastSaved.get(c) === json) return; // nothing new
      c.draftId = await api.saveDraft(c.draftId, c.accountId, doc);
      lastSaved.set(c, json);
    })
      .then(() => this.refreshDrafts())
      .catch((e) => this.showToast({ text: `Couldn't save the draft: ${e}`, error: true }));
  }

  /** Saves the open composer as you type (the composer debounces). */
  autosave(c: ComposerState) {
    if (this.#composer === c) this.#saveDraft(c);
  }

  /** A composer closed or replaced: kept as a draft, or thrown away if it's empty. */
  #putAway(c: ComposerState, next: ComposerState | null, { quiet = false } = {}) {
    // Closed while its conversation shows: the draft stays a card there until you leave.
    if (!quiet && next === null && c.conversationId !== null && this.threadFor === c.conversationId) {
      this.#dismissed = `${c.accountId}:${c.conversationId}`;
    }
    if (worthKeeping(c)) {
      this.#saveDraft(c);
      if (!quiet) this.showToast({ text: "Saved to Drafts" });
      return;
    }
    const kept = new Set(next?.attachments.map((f) => f.id));
    const drop = c.attachments.filter((f) => !kept.has(f.id)).map((f) => f.id);
    if (drop.length) api.removeDraftFiles(drop).catch(() => {});
    if (c.draftId !== null) this.#saveDraft(c); // empty now: removes the draft
  }

  /** Throws away the open composer: its draft and its files. */
  async discardComposer() {
    const c = this.#composer;
    if (!c) return;
    this.#composer = null;
    await this.#queueDraftOp(async () => {
      if (c.draftId !== null) await api.deleteDraft(c.draftId);
      // Files added since the last save aren't in the draft yet.
      if (c.attachments.length) await api.removeDraftFiles(c.attachments.map((f) => f.id));
    }).catch(() => {});
    this.refreshDrafts();
    this.showToast({ text: "Draft discarded" });
  }

  /** Reopens a saved draft in the composer. */
  openDraft(d: SavedDraft) {
    if (this.#composer?.draftId === d.id) return;
    const c: ComposerState = { ...d.draft, accountId: d.accountId, draftId: d.id };
    this.composer = c;
    lastSaved.set(this.composer!, JSON.stringify([c.accountId, draftDoc(c)]));
  }

  /** Discards a saved draft from the Drafts list. */
  async deleteDraft(id: number) {
    if (this.#composer?.draftId === id) return this.discardComposer();
    await this.#queueDraftOp(() => api.deleteDraft(id)).catch((e) => this.showToast({ text: String(e), error: true }));
    await this.refreshDrafts();
    this.showToast({ text: "Draft discarded" });
  }

  async refreshDrafts() {
    try {
      this.drafts = await api.listDrafts();
    } catch {
      return;
    }
    // The Inbox's draft rows follow, the selection staying where it was.
    if (this.#draftRowsHere || this.threads.some((t) => t.draftId !== undefined)) {
      const keep = this.current?.conversationId;
      this.threads = this.#mergeDrafts(this.threads, this.#exhausted);
      const idx = keep ? this.threads.findIndex((t) => t.conversationId === keep) : -1;
      this.selected = idx >= 0 ? idx : Math.min(this.selected, Math.max(0, this.threads.length - 1));
    }
  }

  #openDraftRow(id: number) {
    const d = this.drafts.find((x) => x.id === id);
    if (d) this.openDraft(d);
  }

  // ---------- compose ----------

  compose(prefill: { to?: string; subject?: string; body?: string; attachments?: DraftFile[] } = {}) {
    if (!this.account) return;
    this.composer = { accountId: this.account.id, kind: "new", referenceMessageId: null, conversationId: null, to: prefill.to ?? "", cc: "", bcc: "", subject: prefill.subject ?? "", body: prefill.body ?? "", html: withSignature(textToHtml(prefill.body ?? ""), this.startingSignature(this.account.id, "new")), attachments: prefill.attachments ?? [], droppedAttachments: [], draftId: null };
  }

  /** Reply, reply all or forward the newest message of the current conversation. */
  async respond(kind: Exclude<DraftKind, "new">) {
    const t = this.current;
    // Reply from the account the conversation belongs to.
    const account = this.accountOf(t?.accountId ?? -1);
    if (!t || !account) return;
    if (t.draftId !== undefined) return this.#openDraftRow(t.draftId);
    // Already started one here: that's the reply to carry on with, like Superhuman.
    // Already writing one of these here (r and a share a reply; f a forward): carry on with
    // it, like Superhuman. Something else starts anew, keeping that one as a draft.
    const family = (k: DraftKind) => (k === "forward" ? "forward" : "reply");
    const open = this.#composer;
    if (open && this.composerInline && family(open.kind) === family(kind)) return this.focusComposer();
    const existing = this.draftFor(t.accountId, t.conversationId);
    if (existing && family(existing.draft.kind) === family(kind)) return this.openDraft(existing);
    const msgs = this.threadFor === t.conversationId ? this.thread : await api.getThread(t.accountId, t.conversationId, this.#openedFrom);
    // Reply to the newest message someone else sent, if there is one.
    const me = account.email.toLowerCase();
    const ref = [...msgs].reverse().find((m) => m.fromAddress?.toLowerCase() !== me) ?? msgs.at(-1);
    if (!ref) return;
    const notMe = (a: string | null | undefined): a is string => !!a && a.toLowerCase() !== me;
    const fromIsMe = !notMe(ref.fromAddress);
    let to: string[] = [];
    let cc: string[] = [];
    if (kind === "reply") to = fromIsMe ? ref.to.map((r) => r.address).filter(notMe) : [ref.fromAddress!];
    if (kind === "replyAll") {
      to = [...new Set([ref.fromAddress, ...ref.to.map((r) => r.address)].filter(notMe))];
      cc = ref.cc.map((r) => r.address).filter(notMe);
    }
    const bare = ref.subject.replace(/^((re|fw|fwd):\s*)+/i, "");
    this.composer = {
      accountId: account.id,
      kind,
      referenceMessageId: ref.id,
      conversationId: t.conversationId,
      to: to.join(", "),
      cc: cc.join(", "),
      bcc: "",
      subject: (kind === "forward" ? "FW: " : "RE: ") + bare,
      body: "",
      html: withSignature("", this.startingSignature(account.id, kind)),
      attachments: [],
      droppedAttachments: [],
      draftId: null,
    };
  }

  /** Sends the composer's message after the undo delay, or at `sendAt` when scheduling. */
  async send(sendAt?: Date) {
    const c = this.composer;
    if (!c) return;
    // Sent now, it would go without the file still on its way.
    if (this.pendingFiles > 0) return this.showToast({ text: "Still attaching: send again in a moment", error: true });
    const split = (s: string) => s.split(/[,;]/).map((x) => x.trim()).filter(Boolean);
    this.#sending = c;
    try {
      // A save still on its way would bring the draft back after the send removed it.
      await this.#draftOps;
      const out = await api.sendMessage({
        accountId: c.accountId,
        kind: c.kind,
        referenceMessageId: c.referenceMessageId,
        to: split(c.to),
        cc: split(c.cc),
        bcc: split(c.bcc),
        subject: c.subject,
        body: c.body,
        html: c.html,
        sendAt: sendAt?.toISOString(),
        attachments: c.attachments.map((f) => f.id),
        droppedAttachments: c.kind === "forward" ? c.droppedAttachments : [],
        draftId: c.draftId,
        conversationId: c.conversationId,
      });
      // Not the setter: the outbox has the files now, and the draft goes with the send. And
      // only this one: another composer may have opened while the send was on its way.
      if (this.#composer === c) this.#composer = null;
      this.refreshDrafts();
      const delayMs = new Date(out.sendAt).getTime() - Date.now();
      // Undo works while the toast is up, and never after the message has gone.
      this.#lastSend = { id: out.id, until: Date.now() + (out.scheduled ? TOAST_MS : Math.max(0, delayMs)) };
      const undo = out.scheduled || delayMs > 0;
      this.showToast({ text: out.scheduled ? `Scheduled for ${formatSendAt(out.sendAt)}` : "Sending…", undo });
      this.refreshScheduled();
    } catch (e) {
      this.showToast({ text: String(e), error: true });
    } finally {
      if (this.#sending === c) this.#sending = null;
    }
  }

  /** Takes back a send that hasn't gone and reopens it in the composer. */
  async unsend(id: number): Promise<boolean> {
    const q = await api.cancelSend(id);
    this.refreshScheduled();
    if (!q) return false;
    const d = q.draft;
    this.composer = {
      accountId: q.accountId,
      kind: d.kind,
      referenceMessageId: d.referenceMessageId ?? null,
      // Back under its conversation if that's still the one showing.
      conversationId: this.thread.some((m) => m.id === d.referenceMessageId) ? this.threadFor : null,
      to: d.to.join(", "),
      cc: d.cc.join(", "),
      bcc: (d.bcc ?? []).join(", "),
      subject: d.subject,
      body: d.body,
      html: d.html || textToHtml(d.body),
      attachments: d.attachments ?? [],
      droppedAttachments: d.droppedAttachments ?? [],
      // Back on its draft (out of Drafts while it was being sent).
      draftId: d.draftId ?? null,
    };
    this.refreshDrafts();
    return true;
  }

  /** Cancels a scheduled message without reopening it. */
  async cancelScheduled(id: number) {
    const q = await api.cancelSend(id);
    this.refreshScheduled();
    this.showToast(q ? { text: "Scheduled message cancelled" } : { text: "That message has already been sent", error: true });
  }

  async refreshScheduled() {
    try {
      this.scheduled = await api.scheduledSends();
    } catch {}
  }

  // ---------- misc ----------

  /**
   * Pull the bodies around the cursor into the in-memory cache so opening (or j/k in the
   * reader) skips even the IPC round trip. Rust's background prefetch has usually put them
   * in SQLite already; this covers older mail and anything it hasn't reached yet.
   */
  #schedulePrefetch() {
    clearTimeout(this.#prefetchTimer);
    this.#prefetchTimer = setTimeout(() => {
      const s = this.selected;
      for (const i of [s, s + 1, s + 2, s + 3, s - 1]) {
        const t = this.threads[i];
        if (t && t.draftId === undefined) prefetchBody(t.messageId);
      }
    }, PREFETCH_DELAY_MS);
  }

  async syncNow() {
    try {
      await api.syncNow();
    } catch (e) {
      this.error = String(e);
    }
  }
}

export const mail = new MailStore();

const ref = (t: ThreadRow) => ({ accountId: t.accountId, conversationId: t.conversationId });

/** "Tue 27 Oct, 8:00 am" (today's sends drop the date). */
export function formatSendAt(iso: string, now = new Date()): string {
  const d = new Date(iso);
  const time = d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
  if (d.toDateString() === now.toDateString()) return `today, ${time}`;
  const tomorrow = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
  if (d.toDateString() === tomorrow.toDateString()) return `tomorrow, ${time}`;
  return `${d.toLocaleDateString(undefined, { weekday: "short", day: "numeric", month: "short" })}, ${time}`;
}
