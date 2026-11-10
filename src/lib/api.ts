// Typed wrappers around the Rust commands in src-tauri/src/commands.rs.
// Keep these in sync by hand for now (tauri-specta codegen is a candidate for later).
import type { Recurrence } from "./recurrence";
import type { Snippet } from "./snippets";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type Account = {
  id: number;
  provider: string;
  email: string;
  displayName: string | null;
  /** False after the client ID changes; the cached mail is kept until the same mailbox signs in again. */
  signedIn: boolean;
  /** The OAuth app it signed in with: the one built into this build, or the user's own. */
  oauthApp: OAuthApp;
};

/** "builtin": the app compiled into this build. "custom": the user's own (Settings → Advanced). */
export type OAuthApp = "builtin" | "custom";

export type Folder = {
  id: number;
  accountId: number;
  remoteId: string;
  role: string | null;
  name: string;
  deltaLink: string | null;
  lastSyncedAt: string | null;
};

export type ThreadRow = {
  /** Newest message in the conversation. */
  messageId: number;
  accountId: number;
  conversationId: string;
  subject: string;
  fromName: string | null;
  fromAddress: string | null;
  receivedAt: string;
  preview: string;
  messageCount: number;
  unreadCount: number;
  isFlagged: boolean;
  hasAttachments: boolean;
  /** What the list sorts by: receivedAt, or when a snooze brought it back. */
  sortAt: string;
  /** Back from a snooze. */
  reminder: boolean;
  /** A new-message draft shown in the Inbox among the conversations (its local id). */
  draftId?: number;
  /** For a draft row: who it's to. */
  draftTo?: string;
};

/** A snoozed conversation and when it comes back (UTC). */
export type SnoozedThread = ThreadRow & { until: string };

export type Recipient = { name: string | null; address: string | null };

export type MessageRow = {
  id: number;
  subject: string;
  fromName: string | null;
  fromAddress: string | null;
  to: Recipient[];
  cc: Recipient[];
  receivedAt: string;
  preview: string;
  isRead: boolean;
  hasAttachments: boolean;
  isFlagged: boolean;
  /** Folder holding this copy: inbox, sentitems, archive, ... */
  folderRole: string;
  /** "message", or "eventMessage" for meeting invites and responses. */
  messageType: string;
  /** meetingRequest | meetingCancelled | meetingAccepted | meetingTenativelyAccepted (Graph's spelling) | meetingDeclined */
  meetingMessageType: string | null;
};

/**
 * `moveToInbox`: archived or deleted mail back to the inbox (e in Done, # in Trash).
 * `spam`: to Junk (Gmail: report spam); `notSpam`: from Junk back to the inbox.
 */
export type Action = "archive" | "trash" | "moveToInbox" | "spam" | "notSpam" | "markRead" | "markUnread" | "flag" | "unflag";
export type ActionOutcome = { label: string | null; changed: number };
/** A conversation, as the batch commands take it. */
export type ThreadRef = { accountId: number; conversationId: string };
export type DraftKind = "new" | "reply" | "replyAll" | "forward";

/** The mail views the sidebar lists, each across signed-in accounts. */
export type Mailbox = "inbox" | "done" | "sent" | "snoozed" | "all" | "trash" | "junk";
export type SendInput = {
  accountId: number;
  kind: DraftKind;
  referenceMessageId: number | null;
  to: string[];
  cc: string[];
  bcc: string[];
  subject: string;
  /** The text as plain text. */
  body: string;
  /** The text as rich text (the composer's HTML). */
  html: string;
  /** Send later (UTC ISO). Omitted: after the undo delay. */
  sendAt?: string;
  /** Ids of files from `addDraftFile`. */
  attachments?: number[];
  /** Forwards: names of the original's attachments to leave out. */
  droppedAttachments?: string[];
  /** The saved draft this is (sending removes it). */
  draftId?: number | null;
  /** The conversation it answers (kept with its draft until it has gone). */
  conversationId?: string | null;
};
/** The composer's state as a draft keeps it. */
export type DraftDoc = {
  kind: DraftKind;
  referenceMessageId: number | null;
  conversationId: string | null;
  to: string;
  cc: string;
  bcc: string;
  subject: string;
  body: string;
  html: string;
  attachments: DraftFile[];
  droppedAttachments: string[];
  /** Started in Outlook or Gmail (sent through the server's copy). */
  imported?: boolean;
};
/** A saved draft. */
export type SavedDraft = { id: number; accountId: number; updatedAt: string; draft: DraftDoc };
/** A file attached in the composer (its bytes wait in Rust until the message has gone). */
export type DraftFile = { id: number; name: string; mime: string; size: number };
export type SendOutcome = { id: number; sendAt: string; scheduled: boolean };
export type Draft = {
  kind: DraftKind;
  to: string[];
  cc: string[];
  bcc?: string[];
  subject: string;
  body: string;
  /** Missing on drafts queued before the rich-text composer: `body` is all there is. */
  html?: string;
  sendAt?: string;
  referenceMessageId?: number;
  attachments?: DraftFile[];
  droppedAttachments?: string[];
  /** The saved draft it was, back in Drafts if the send is taken back. */
  draftId?: number;
};
/**
 * An account's signature, keyed by its address. Added to new messages, and to replies and
 * forwards when `replies`. `managed`: the organisation's mail server adds one (CodeTwo,
 * Exclaimer...), so PeetMail adds none.
 */
export type Signature = { address: string; html: string; replies: boolean; managed: boolean };
/** A send that hasn't gone out yet. */
export type QueuedSend = { id: number; accountId: number; sendAt: string; draft: Draft };
export type Contact = { name: string | null; address: string };
/** `id`: stable through renames and reorders; absent on a split not saved yet. */
export type Split = { id?: string; name: string; query: string };
/** A tab of the split inbox. The first is the built-in Inbox (`builtin`): mail no split claims. */
export type SplitInfo = Split & { builtin: boolean; count: number; unread: number };
export type Attachment = { id: string; name: string; contentType: string; size: number };
export type OutboxStatus = { pending: number; failed: number; lastError: string | null };

export type CacheLimits = { cacheDays: number; bodyCacheMb: number };

export type Layout = "list" | "preview";
export type Preferences = {
  hideEmptySplits: boolean;
  showRightPane: boolean;
  rightPaneWidth: number;
  /** "list": Enter opens a conversation. "preview": list with a reading pane beside it. */
  layout: Layout;
  listWidth: number;
  /** What the right pane shows. */
  rightPaneTab: "recent" | "calendar";
  /** First day of the week: 0 = Sunday, 1 = Monday ... 6 = Saturday. */
  weekStartsOn: number;
  /** Seconds a sent message waits so it can be undone (0: at once). */
  undoSendSecs: number;
  /** New-mail notifications. */
  notify: "off" | "focused" | "all";
  /** Closing the window keeps PeetMail running in the tray. */
  closeToTray: boolean;
};

export type Attendee = { name: string | null; address: string | null; response: string | null; type: string | null };

export type CalendarEvent = {
  id: number;
  accountId: number;
  remoteId: string;
  subject: string;
  /** UTC ISO instant, or YYYY-MM-DD for all-day events. */
  startAt: string;
  /** Exclusive: UTC ISO instant, or YYYY-MM-DD for all-day events. */
  endAt: string;
  isAllDay: boolean;
  location: string | null;
  organizerName: string | null;
  organizerAddress: string | null;
  attendees: Attendee[];
  /** none | organizer | accepted | tentativelyAccepted | declined | notResponded */
  response: string | null;
  isOrganizer: boolean;
  isCancelled: boolean;
  showAs: string | null;
  joinUrl: string | null;
  webLink: string | null;
  preview: string;
  /** "Microsoft Teams", "Zoom", "Google Meet", "Webex" or "Online meeting" (with joinUrl). */
  joinProvider: string | null;
  /** singleInstance | occurrence | exception */
  eventType: string;
  /** The series an occurrence belongs to. */
  seriesMasterId: string | null;
};

/** What the event popup shows beyond the cached row; fetched when it opens. */
export type EventDetail = {
  bodyHtml: string;
  recurrence: Recurrence | null;
  seriesMasterId: string | null;
  isOccurrence: boolean;
  isException: boolean;
  /** null: no reminder (unless reminderDefault). */
  reminderMinutes: number | null;
  reminderDefault: boolean;
  sensitivity: string;
  categories: string[];
  importance: string;
  /** A join link found in the description (when the event has none). */
  joinUrl: string | null;
  joinProvider: string | null;
};

export type Bootstrap = {
  /** The user's own Microsoft app (Settings → Advanced), if set. */
  clientId: string | null;
  /** The user's own Google OAuth client ID, if set. */
  googleClientId: string | null;
  /** Providers with an app built into this build: signing in needs no setup. */
  builtin: { microsoft: boolean; google: boolean };
  accounts: Account[];
  cacheLimits: CacheLimits;
  preferences: Preferences;
};
export type Provider = "microsoft" | "google";

/** A conversation from the Recent Opens pane: its newest message, plus when it was opened. */
export type RecentOpen = ThreadRow & { openedAt: string };

export type Cursor = { sortAt: string; messageId: number };

export type SyncStatus = { state: "syncing" | "idle" | "error"; message: string | null };

/** What a server search found. */
export type ServerSearchOutcome = {
  /** Messages found, across accounts. */
  found: number;
  /** Accounts searched. */
  searched: number;
  /** Addresses of accounts whose search failed. */
  failed: string[];
};

export const api = {
  bootstrap: () => invoke<Bootstrap>("bootstrap"),
  setClientId: (clientId: string) => invoke<void>("set_client_id", { clientId }),
  /** `oauthApp` unset: the built-in app when the build has one, else the user's own. */
  /** `loginHint`: the address to suggest on the sign-in page (an account from another PC). */
  signIn: (provider: Provider = "microsoft", oauthApp?: OAuthApp, loginHint?: string) =>
    invoke<Account>("sign_in", { provider, oauthApp: oauthApp ?? null, loginHint: loginHint ?? null }),
  setGoogleClient: (clientId: string, clientSecret: string | null) => invoke<void>("set_google_client", { clientId, clientSecret }),
  cancelSignIn: () => invoke<void>("cancel_sign_in"),
  /** `everywhere` (settings sync): sign it out on every PC that syncs, not just this one. */
  signOut: (accountId: number, everywhere = false) => invoke<void>("sign_out", { accountId, everywhere }),
  listFolders: (accountId: number) => invoke<Folder[]>("list_folders", { accountId }),
  /**
   * Conversations in `mailbox` (null: all mail, for search). `split` picks a split-inbox tab
   * (0 = Inbox) and wins over both; `query` uses the query language (src-tauri/src/query.rs).
   * `hidden`: accounts toggled off in the footer legend. `withServer`: a search also lists
   * what the server search (`searchServer`) found.
   */
  listThreads: (mailbox: Mailbox | null, query: string, split: number | null, cursor: Cursor | null, limit: number, hidden: number[] = [], withServer = false) =>
    invoke<ThreadRow[]>("list_threads", { mailbox, query, split, cursor, limit, hidden, withServer }),
  /** Searches the server for mail outside the cache window; results join the search's list until `clearServerSearch`. */
  searchServer: (query: string, hidden: number[] = []) => invoke<ServerSearchOutcome>("search_server", { query, hidden }),
  /** Fetches the rest of a conversation opened from server results; true when anything came. */
  fetchSearchConversation: (accountId: number, conversationId: string) =>
    invoke<boolean>("fetch_search_conversation", { accountId, conversationId }),
  /** Closes the server search: mail it fetched leaves the cache again. */
  clearServerSearch: () => invoke<void>("clear_server_search"),
  /** `from`: the view it's opened from. Messages in Deleted Items and Junk only show from Trash and Junk. */
  getThread: (accountId: number, conversationId: string, from: Mailbox | null = null) =>
    invoke<MessageRow[]>("get_thread", { accountId, conversationId, from }),
  getMessageBody: (messageId: number) => invoke<string>("get_message_body", { messageId }),
  syncNow: () => invoke<void>("sync_now"),
  setCacheLimits: (limits: CacheLimits) => invoke<void>("set_cache_limits", { limits }),
  setPreferences: (preferences: Preferences) => invoke<void>("set_preferences", { preferences }),
  recordOpen: (accountId: number, conversationId: string) => invoke<void>("record_open", { accountId, conversationId }),
  recentOpens: (limit = 50) => invoke<RecentOpen[]>("recent_opens", { limit }),
  /** Timed events by UTC instants [from, to); all-day events by local dates [fromDate, toDate). */
  listEvents: (from: string, to: string, fromDate: string, toDate: string) =>
    invoke<CalendarEvent[]>("list_events", { from, to, fromDate, toDate }),
  inviteEvent: (messageId: number) => invoke<Invite | null>("invite_event", { messageId }),
  listRooms: (accountId: number) => invoke<RoomList>("list_rooms", { accountId }),
  /** The online meeting new events on this account can have ("teamsForBusiness", "hangoutsMeet"...), or null. */
  meetingProvider: (accountId: number) => invoke<string | null>("meeting_provider", { accountId }),
  /** Sign a known account in again in place (new tokens, optionally extra permissions); keeps its mail. */
  reconnectAccount: (accountId: number, addScopes?: string[]) => invoke<Account>("reconnect_account", { accountId, addScopes: addScopes ?? null }),
  createEvent: (accountId: number, event: NewEvent) => invoke<CalendarEvent>("create_event", { accountId, event }),
  eventDetail: (accountId: number, remoteId: string) => invoke<EventDetail>("event_detail", { accountId, remoteId }),
  eventForEdit: (accountId: number, remoteId: string, series: boolean) =>
    invoke<EditableEvent>("event_for_edit", { accountId, remoteId, series, timeZone: Intl.DateTimeFormat().resolvedOptions().timeZone }),
  updateEvent: (accountId: number, remoteId: string, changes: EventChanges) => invoke<void>("update_event", { accountId, remoteId, changes }),
  deleteCalendarEvent: (accountId: number, remoteId: string, cancel: boolean, comment: string | null) =>
    invoke<void>("delete_calendar_event", { accountId, remoteId, cancel, comment }),
  respondToEvent: (accountId: number, eventRemoteId: string, response: Rsvp, series = false) =>
    invoke<void>("respond_to_event", { accountId, eventRemoteId, response, series }),
  snoozeThread: (accountId: number, conversationId: string, until: string) =>
    invoke<ActionOutcome>("snooze_thread", { accountId, conversationId, until }),
  /** Snoozes several conversations as one undo step. */
  snoozeThreads: (targets: ThreadRef[], until: string) => invoke<ActionOutcome>("snooze_threads", { targets, until }),
  unsnoozeThread: (accountId: number, conversationId: string) => invoke<void>("unsnooze_thread", { accountId, conversationId }),
  listSnoozed: () => invoke<SnoozedThread[]>("list_snoozed"),
  listSnippets: () => invoke<Snippet[]>("list_snippets"),
  getAutostart: () => invoke<boolean>("get_autostart"),
  setAutostart: (enabled: boolean) => invoke<void>("set_autostart", { enabled }),
  quitApp: () => invoke<void>("quit_app"),
  /** Opens a link in the browser / mail app (recorded instead in tests). */
  openExternal: (url: string) => invoke<void>("open_external", { url }),
  setSnippets: (snippets: Snippet[]) => invoke<Snippet[]>("set_snippets", { snippets }),
  listSignatures: () => invoke<Signature[]>("list_signatures"),
  setSignatures: (signatures: Signature[]) => invoke<Signature[]>("set_signatures", { signatures }),
  threadAction: (accountId: number, conversationId: string, action: Action, undoable = true) =>
    invoke<ActionOutcome>("thread_action", { accountId, conversationId, action, undoable }),
  /** Acts on several conversations (a selection) as one undo step. */
  threadsAction: (targets: ThreadRef[], action: Action) => invoke<ActionOutcome>("threads_action", { targets, action }),
  undo: () => invoke<string | null>("undo"),
  outboxStatus: () => invoke<OutboxStatus>("outbox_status"),
  sendMessage: (input: SendInput) => invoke<SendOutcome>("send_message", { input }),
  /** Hands a file's bytes to Rust as the raw request body. */
  addDraftFile: async (file: File) =>
    invoke<DraftFile>("add_draft_file", new Uint8Array(await file.arrayBuffer()), {
      headers: { "x-file-name": encodeURIComponent(file.name), "x-file-type": file.type },
    }),
  removeDraftFiles: (ids: number[]) => invoke<void>("remove_draft_files", { ids }),
  /** Saves (or updates, with its id) a draft. Returns its id. */
  saveDraft: (id: number | null, accountId: number, draft: DraftDoc) => invoke<number>("save_draft", { id, accountId, draft }),
  listDrafts: () => invoke<SavedDraft[]>("list_drafts"),
  /** Discards a draft and its files. */
  deleteDraft: (id: number) => invoke<void>("delete_draft", { id }),
  cancelSend: (id: number) => invoke<QueuedSend | null>("cancel_send", { id }),
  scheduledSends: () => invoke<QueuedSend[]>("scheduled_sends"),
  suggestContacts: (prefix: string) => invoke<Contact[]>("suggest_contacts", { prefix }),
  /** `folderId` null: the unified inbox (every signed-in account). */
  listSplits: (folderId: number | null, hidden: number[] = []) => invoke<SplitInfo[]>("list_splits", { folderId, hidden }),
  setSplits: (splits: Split[]) => invoke<void>("set_splits", { splits }),
  getMessageBodyWithImages: (messageId: number) => invoke<string>("get_message_body_with_images", { messageId }),
  listAttachments: (messageId: number) => invoke<Attachment[]>("list_attachments", { messageId }),
  privacy: () => invoke<Privacy>("privacy"),
  setCrashReports: (enabled: boolean) => invoke<void>("set_crash_reports", { enabled }),
  dismissPrivacyNotice: () => invoke<void>("dismiss_privacy_notice"),
  changelogSeen: () => invoke<string[] | null>("changelog_seen"),
  setChangelogSeen: (ids: string[]) => invoke<void>("set_changelog_seen", { ids }),
  setUsageData: (enabled: boolean) => invoke<void>("set_usage_data", { enabled }),
  uiEvents: (events: import("./perf").UiEvent[]) => invoke<void>("ui_events", { events }),
  reportError: (name: string, message: string, stack: string | null, mechanism: string) =>
    invoke<void>("report_error", { name, message, stack, mechanism }),
  updateStatus: () => invoke<UpdateStatus>("update_status"),
  checkForUpdates: () => invoke<UpdateStatus>("check_for_updates"),
  installUpdate: () => invoke<void>("install_update"),
  setUpdateChannel: (channel: UpdateChannel) => invoke<UpdateStatus>("set_update_channel", { channel }),
  downloadAttachment: (messageId: number, attachmentId: string, name: string) =>
    invoke<string>("download_attachment", { messageId, attachmentId, name }),
  /** Opens an attachment in its default app (refused for files that run programs). */
  openAttachment: (messageId: number, attachmentId: string, name: string) =>
    invoke<void>("open_attachment", { messageId, attachmentId, name }),
  healthIssues: () => invoke<HealthIssue[]>("health_issues"),
  feedbackOptions: () => invoke<FeedbackOptions>("feedback_options"),
  /** Saves the diagnostics zip to Downloads; returns its path. */
  exportDiagnostics: () => invoke<string>("export_diagnostics"),
  sendFeedback: (message: string, email: string | null, includeDiagnostics: boolean) =>
    invoke<void>("send_feedback", { message, email, includeDiagnostics }),
};

/** Something outside PeetMail the startup health checks found (health.rs). */
export interface HealthIssue {
  id: "db_integrity" | "disk_space" | "data_dir" | "clock" | "webview";
  severity: "error" | "warning";
  title: string;
  detail: string;
}

export const onHealthChanged = (cb: (issues: HealthIssue[]) => void): Promise<UnlistenFn> =>
  listen<HealthIssue[]>("health-changed", (e) => cb(e.payload));

/** What Help → Report a problem can do in this build. */
export interface FeedbackOptions {
  /** Feedback can be sent to the developers from the app. */
  sentry: boolean;
  /** The address "Prefer email?" writes to, if this build has one. */
  email: string | null;
}

export const onMailChanged = (cb: (p: { accountId: number; folderId: number }) => void): Promise<UnlistenFn> =>
  listen<{ accountId: number; folderId: number }>("mail-changed", (e) => cb(e.payload));

export const onOutbox = (cb: (s: OutboxStatus) => void): Promise<UnlistenFn> =>
  listen<OutboxStatus>("outbox-changed", (e) => cb(e.payload));

export type Rsvp = "accept" | "tentative" | "decline";
/** A new event: timed ones as UTC ISO instants, all-day ones as YYYY-MM-DD (end exclusive). */
export type NewEvent = {
  subject: string;
  start: string;
  end: string;
  allDay: boolean;
  location: string | null;
  /** IANA zone the all-day dates belong to. */
  timeZone: string;
  /** A full event's other fields (the editor). */
  details?: EventChanges;
};

export type EventTimes = {
  /** Timed: UTC ISO instants. All-day: YYYY-MM-DD (end exclusive). */
  start: string;
  end: string;
  allDay: boolean;
  timeZone: string;
  /** Wall-clock start/end in timeZone (YYYY-MM-DDTHH:MM:SS), sent for a series so it keeps its local time. */
  localStart?: string;
  localEnd?: string;
};
export type EditAttendee = { name: string | null; address: string; optional: boolean; response?: string | null; /** A room being booked. */ resource?: boolean };
/** A meeting room of a Microsoft 365 tenant. */
export type Room = { name: string; address: string; capacity: number | null; building: string | null };
/** Rooms, or (when there are none) the permission that would list them. */
export type RoomList = { rooms: Room[]; needs: { scope: string; admin: boolean } | null };
/** What an edit changes; missing fields stay as they are. */
export type EventChanges = {
  subject?: string;
  times?: EventTimes;
  /** "" clears it. */
  location?: string;
  /** The room's mailbox when the location is a room. */
  locationEmail?: string;
  descriptionHtml?: string;
  attendees?: EditAttendee[];
  showAs?: string;
  reminder?: { minutes: number | null };
  sensitivity?: "normal" | "private";
  /** null stops it repeating. */
  recurrence?: Recurrence | null;
  /** Adds an online meeting from this provider (see `meetingProvider`). */
  onlineMeeting?: string;
};
/** The event the editor works on (fresh from the server). */
export type EditableEvent = {
  remoteId: string;
  subject: string;
  times: EventTimes;
  location: string;
  locationEmail: string | null;
  descriptionHtml: string;
  attendees: EditAttendee[];
  showAs: string;
  reminderMinutes: number | null;
  reminderDefault: boolean;
  sensitivity: "normal" | "private";
  recurrence: Recurrence | null;
  /** It already has an online meeting (Teams, Meet...). */
  onlineMeeting: boolean;
  isOrganizer: boolean;
  series: boolean;
};
/** The calendar event behind a meeting-invite message. */
export type Invite = {
  meetingMessageType: string | null;
  /** newMeetingRequest | fullUpdate | informationalUpdate | silentUpdate | outdated | ... */
  meetingRequestType: string | null;
  /** Where an updated meeting used to be (UTC instants / text). */
  previousStart: string | null;
  previousEnd: string | null;
  previousLocation: string | null;
  /** How the series repeats; `event` is then the series master (its first occurrence's times). */
  recurrence: Recurrence | null;
  event: CalendarEvent;
};

export const onScheduledChanged = (cb: () => void): Promise<UnlistenFn> => listen("scheduled-changed", () => cb());

/** A sync brought drafts in from Outlook or Gmail (new, changed or deleted there). */
export const onDraftsChanged = (cb: () => void): Promise<UnlistenFn> => listen("drafts-changed", () => cb());

export const onCalendarChanged = (cb: () => void): Promise<UnlistenFn> => listen("calendar-changed", () => cb());

/** Settings → Privacy (src-tauri/src/crash.rs). */
export type Privacy = {
  configured: boolean;
  crashReports: boolean;
  usageConfigured: boolean;
  usageData: boolean;
  installId: string;
  noticeSeen: boolean;
};

/** Automatic updates (src-tauri/src/update.rs). */
export type UpdateChannel = "stable" | "canary";
export type UpdateStatus = {
  /** Increases with every change; a lower one is stale. */
  seq: number;
  state: "disabled" | "idle" | "checking" | "downloading" | "ready" | "upToDate" | "error";
  channel: UpdateChannel;
  currentVersion: string;
  /** The version found (downloading or ready). */
  version: string | null;
  notes: string | null;
  message: string | null;
  lastCheck: string | null;
};

export const onUpdateStatus = (cb: (s: UpdateStatus) => void): Promise<UnlistenFn> =>
  listen<UpdateStatus>("update-status", (e) => cb(e.payload));

export const onSyncStatus = (cb: (s: SyncStatus) => void): Promise<UnlistenFn> =>
  listen<SyncStatus>("sync-status", (e) => cb(e.payload));

// ---------- settings sync (src-tauri/src/sync, docs/SYNC.md) ----------

/** How a mail account stands with the sync account. */
export type SyncLinkState = "linked" | "elsewhere" | "unlinked" | "failed" | "pending";
export type SettingsSyncStatus = {
  state: "off" | "idle" | "syncing" | "error";
  server: string | null;
  lastSyncedAt: string | null;
  error: string | null;
  /** A sign-in waiting for its next step. */
  pending: "join" | "create" | null;
  accounts: { accountId: number; email: string; state: SyncLinkState }[];
};
export type SyncConnected = { next: "join" | "create"; accounts: { id: number; email: string }[] };
export type SyncDevice = { id: number; name: string; createdAt: string; lastSeenAt: string; current: boolean };
export type SyncIdentity = { id: number; provider: "microsoft" | "google"; email: string | null; linkedAt: string };

/** An account another PC syncs that this one doesn't have. */
export type ElsewhereAccount = { provider: Provider; email: string; oauthApp: OAuthApp; displayName: string | null };

export const settingsSyncApi = {
  status: () => invoke<SettingsSyncStatus>("settings_sync_status"),
  /** Accounts on other PCs that sync, not on this one (none while sync is off). */
  elsewhere: () => invoke<ElsewhereAccount[]>("synced_accounts"),
  dismissElsewhere: (provider: Provider, email: string) => invoke<void>("dismiss_synced_account", { provider, email }),
  connect: (server: string) => invoke<SyncConnected>("settings_sync_connect", { server }),
  /** The passphrase or the recovery key. */
  join: (secret: string) => invoke<SettingsSyncStatus>("settings_sync_join", { secret }),
  create: (accountId: number, passphrase: string) => invoke<{ recoveryKey: string }>("settings_sync_create", { accountId, passphrase }),
  cancel: () => invoke<void>("settings_sync_cancel"),
  syncNow: () => invoke<SettingsSyncStatus>("settings_sync_now"),
  devices: () => invoke<SyncDevice[]>("settings_sync_devices"),
  revoke: (deviceId: number) => invoke<void>("settings_sync_revoke", { deviceId }),
  identities: () => invoke<SyncIdentity[]>("settings_sync_identities"),
  unlink: (identityId: number) => invoke<void>("settings_sync_unlink", { identityId }),
  changePassphrase: (passphrase: string) => invoke<void>("settings_sync_change_passphrase", { passphrase }),
  recoveryKey: () => invoke<string>("settings_sync_recovery_key"),
  disable: () => invoke<void>("settings_sync_disable"),
  deleteAccount: () => invoke<void>("settings_sync_delete_account"),
};

export const onSettingsSyncStatus = (cb: (s: SettingsSyncStatus) => void): Promise<UnlistenFn> =>
  listen<SettingsSyncStatus>("settings-sync-status", (e) => cb(e.payload));

/** Settings another device changed (`keys`), so whatever shows them reloads. */
export const onSettingsChanged = (cb: (keys: string[]) => void): Promise<UnlistenFn> =>
  listen<{ keys: string[] }>("settings-changed", (e) => cb(e.payload.keys));
