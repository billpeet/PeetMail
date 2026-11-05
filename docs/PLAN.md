# PeetMail plan

A Superhuman-style mail and calendar client for Windows and Linux. Personal use, Microsoft 365 and Outlook.com first, Gmail later.

## Principles

1. **Fast.** No interaction waits on the network. Reads come from a local SQLite cache, writes are optimistic, and the next likely thread is already rendered.
2. **Minimal.** Every feature has to earn its place. When unsure, leave it out.
3. **Keyboard first.** Every action has a shortcut and shows up in the command palette. The mouse works but is never required.

## Decisions

| Decision | Choice | Why |
|---|---|---|
| Shell | Tauri 2 | Email bodies need a web engine anyway. Tauri gives WebView2 plus a Rust core, without the weight of Electron. |
| UI | SvelteKit SPA (adapter-static), Svelte 5 runes, Tailwind 4 | Fine-grained reactivity, small runtime, matches the usual stack. |
| Storage | SQLite via `rusqlite` in the Rust process | Sync, search and caching never cross IPC. Drizzle doesn't fit because the DB lives in Rust, not JS. |
| Providers | Microsoft Graph first, Gmail API second | M365 + Outlook.com are the daily drivers. |
| Auth | Entra public client, auth code + PKCE, loopback redirect | No client secret in the binary. Works for both work and personal accounts via the `common` authority. |
| Token storage | DPAPI-encrypted blob in SQLite | Credential Manager caps blobs at 2.5KB, and Microsoft refresh tokens can be bigger. |
| Message IDs | Graph immutable IDs (`Prefer: IdType="ImmutableId"`) | IDs survive folder moves, so archive and undo don't orphan cache rows. |
| Sync | Delta queries, polled every 60s | Graph push notifications need a public webhook. Delta polls cost one request per folder when nothing changed. |
| Platform | Windows, and Linux as an AppImage (see `docs/DISTRIBUTION.md`, Stage 7) | Tauri keeps macOS open later at low cost. |
| IPC types | Hand-written TS types in `src/lib/api.ts` | Command count is small. Revisit `tauri-specta` once it passes ~25 commands. |

## Architecture

```
┌──────────────── Svelte UI (WebView2) ─────────────────┐
│ keymap registry · views · virtual list · body LRU     │
│ sandboxed iframe for message bodies                   │
└──────────── invoke() commands + emitted events ───────┘
┌──────────────────── Rust core ────────────────────────┐
│ auth.rs     PKCE sign-in, token refresh               │
│ graph.rs    Graph HTTP client, 429/503 backoff        │
│ mail.rs     delta sync, body fetch                    │
│ db.rs       SQLite schema, queries, cache eviction    │
│ sanitize.rs ammonia HTML cleaning                     │
│ secure.rs   DPAPI                                     │
│ state.rs    app state, settings, token cache          │
└───────────────────────────────────────────────────────┘
```

Events from Rust to UI:
- `mail-changed {accountId, folderId}` after each committed sync page. The UI debounces it and reloads what it has loaded.
- `sync-status {state, message}` drives the footer indicator.

### Data model

- `accounts` holds provider, email and the encrypted token blob.
- `folders` holds the Graph folder id, a role (inbox, sentitems, ...) and the delta link.
- `messages` holds metadata only. Threads are `conversation_id` groups.
- `messages_fts` is the FTS5 search index, kept in step by triggers; body text is added when a body is fetched.
- `outbox` holds queued Graph mutations (see `outbox.rs`).
- `message_bodies` holds sanitized HTML, with `size` and `accessed_at` for LRU eviction.
- `settings` is key/value.

Migrations are numbered SQL files in `src-tauri/migrations`, applied in order and tracked in `PRAGMA user_version`.

### Cache limits

Both are in settings (`Ctrl+,`):
- **Mail window** (default 90 days). The initial delta request filters on `receivedDateTime ge <cutoff>`. After each sync, messages older than the cutoff are pruned. Widening the window clears delta links and resyncs. If Graph rejects the filter, sync falls back to filtering locally.
- **Body cache** (default 500 MB). Bodies are fetched on demand and prefetched around the cursor. After each insert, the least recently read bodies are evicted until the total fits.

Metadata for 90 days of a busy inbox is a few MB, so it isn't capped separately.

### Rendering email safely

Three layers:
1. `ammonia` strips scripts, event handlers and `javascript:` URLs, and keeps layout markup and inline styles.
2. The body renders in an iframe with `sandbox="allow-same-origin"`, so no script inside it can run.
3. The srcdoc iframe inherits the app CSP. `img-src 'self' data:` blocks remote images and tracking pixels by default.

The parent page handles link clicks, opening them in the system browser, and forwards keydowns from the iframe so shortcuts keep working after you click into a message.

## Keyboard map

Implemented bindings are marked ✓; numbers are the phase that adds them. They follow Superhuman's defaults.

| Keys | Action | |
|---|---|---|
| `j` / `k`, `↓` / `↑` | Next / previous conversation | ✓ |
| `Enter` | Open conversation | ✓ |
| `Esc` | Back to list, close dialog | ✓ |
| `g g` / `G` | Top / bottom of list | ✓ |
| `g i` | Go to inbox | ✓ |
| `g e` / `g t` / `g a` / `g #` | Done / Sent / All mail / Trash (`e` in Done and `#` in Trash: back to the inbox) | ✓ |
| `Ctrl+R` | Sync now | ✓ |
| `Ctrl+,` | Settings | ✓ |
| `?` | Shortcut overlay | ✓ |
| `Ctrl+K` | Command palette | ✓ |
| `e` | Archive (mark done) | ✓ |
| `#` | Delete | ✓ |
| `u` | Toggle read | ✓ |
| `s` | Flag | ✓ |
| `h` | Snooze: presets (later today, this evening, tomorrow, weekend, next week) or a typed time | ✓ |
| `g h` | Snoozed, a list view (`Enter` read, `h` change time, `u` back now, `e`/`#` archive/delete; undo returns them to Snoozed) | ✓ |
| `r` / `a` / `f` | Reply / reply all / forward | ✓ |
| `c` | Compose | ✓ |
| `/` | Search | ✓ |
| `Tab` / `Shift+Tab` | Next / previous split | ✓ |
| `z` | Undo last action (also takes back a send during its undo window) | ✓ |
| `Ctrl+Shift+Enter` | Send later, in the composer ("tomorrow 9am", "mon", "in 2 hours", or a preset) | ✓ |
| `;name`, `Ctrl+;` | Insert a snippet, in the composer | ✓ |
| `Ctrl+Shift+S` | Remove or restore the signature, in the composer | ✓ |
| `Ctrl+Q` | Quit (closing the window keeps PeetMail in the tray) | ✓ |
| `e` / `#` | In an event's details: edit / delete (a recurring event asks: this occurrence `o` or the series `s`) | ✓ |
| `Tab` | In quick add: open the full event editor (guests, location, description) | ✓ |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | In quick add: next / previous calendar | ✓ |
| `g s` | Scheduled, a list view (`Enter` edit, `#` cancel) | ✓ |
| `i` | Load remote images in the open conversation | ✓ |
| `\` | Show / hide the right pane (Recent Opens) | ✓ |
| `v` | Toggle the reading pane (list only, or list + preview) | ✓ |
| `g c` | Calendar (week view, first day set in Settings; ←/→ change week, `t` this week, Esc back) | ✓ |
| `1` / `2` | Mail / Calendar (also the left sidebar) | ✓ |
| `Shift+C` | New event (quick add: "lunch with Sam fri 1pm @ Café") | ✓ |
| `0` | Toggle calendar side panel | ✓ |

Every binding lives in one registry (`src/lib/keys.svelte.ts`) with an id, label, group and `when` condition. The shortcut overlay reads from it today, and the command palette will too.

## Phases

### Phase 0: skeleton (done)

- [x] Tauri 2 + SvelteKit + Tailwind 4 scaffold, CSP configured
- [x] Entra sign-in with PKCE over loopback, DPAPI token storage, silent refresh
- [x] SQLite schema and migrations
- [x] Inbox delta sync with date window, removal handling, 410 resync, throttling backoff
- [x] Background sync every 60s, `Ctrl+R` to sync now
- [x] Virtualized conversation list with keyset pagination
- [x] Thread reader with sanitized bodies in a sandboxed iframe
- [x] Body prefetch around the cursor, JS-side LRU over the SQLite body cache
- [x] Keymap registry with sequences (`g i`), shortcut overlay
- [x] Configurable cache limits
- [ ] First real sign-in and sync against your mailbox (needs your Entra app, see below)

### Phase 1: the core loop (done)

- [x] Outbox: archive, delete, read/unread and flag apply to SQLite first and queue a Graph op. Triage ops wait 2s before sending, so a quick undo never touches the server. Undo (`z`) cancels queued ops or queues the inverse. Retries with backoff; permanent failures show a toast and a "failed" count.
- [x] Sync leaves messages with queued ops alone, so a stale delta can't revert a local change.
- [x] Opening a conversation marks it read. Archive or delete in the reader moves on to the next conversation.
- [x] Compose, reply, reply all, forward (`c` `r` `a` `f`, `Ctrl+Enter` sends). Plain text in, HTML out. Replies and forwards go through Graph's `reply`/`replyAll`/`forward` so the original is quoted and threaded. Contact suggestions from people you've exchanged mail with.
- [x] One query language for search and splits: `from:` `to:` `subject:` `is:unread|read|flagged|focused|other|invite` `has:attachment`, `-` to negate, free words as prefixes, `"quoted phrases"`.
- [x] Search (`/`): SQLite FTS5 over subject, people, preview, and body text once a body has been fetched. Covers all cached folders except Deleted Items.
- [x] Split inboxes: named queries, `Tab`/`Shift+Tab` to cycle, unread counts on tabs, editable in Settings. A built-in Inbox tab comes first and holds whatever no split claims; each message lands in exactly one tab (first matching split wins). Default split: Other (`is:other`).
- [x] Command palette (`Ctrl+K`) over the keymap registry.
- [x] Sent Items synced, so threads include your replies. Archive and Deleted Items synced too since the Folders work (below).
- [x] Load images (`i`): Rust fetches remote images (raster only, 5 MB cap) and inlines them as data URIs; the CSP stays strict.
- [x] Inline `cid:` images, attachment list, download to Downloads.

Deferred from Phase 1:
- [x] Server search for mail outside the cache window (see "Searching the server" under Tier 1).
- Syncing Drafts; a folder picker / move-to-folder.
- Markdown shortcuts in the composer.

### Phase 2: calendar

- Calendar sync via `/me/calendarView/delta`
- Day view side panel next to the inbox, plus a full calendar view (`g c`)
- RSVP to invites inline in the message
- Quick add in natural language ("lunch with Sam fri 1pm")

### Phase 3: polish

- [x] Snooze (`h`). The conversation moves to a hidden "PeetMail Snoozed" server folder (visible if the mailbox refuses hidden folders), so other clients stop showing it too. PeetMail wakes it on time (a 1s local check; on next start if it was closed): back to the inbox, newest message unread, sorted on top by its wake time (`messages.sort_at`) with a Reminder badge. Undoable. Not done: "remind me if nobody replies".
- [x] Undo send (delay window, 10s by default, set in Settings), scheduled send. Both wait in the local outbox, so a scheduled message only goes out while PeetMail is running (or as soon as it next starts). Server-side deferred delivery (`PR_DEFERRED_SEND_TIME`) would send with the PC off, but needs the draft-then-send flow; revisit if that matters.
- [x] Snippets: made in Settings (name + text); in the composer type `;name` or press `Ctrl+;`, pick with ↑/↓, Enter inserts. Placeholders `{first_name}` `{name}` (first recipient; guessed from the address when there's no name) and `{my_first_name}` `{my_name}`. Stored in settings as JSON.
- [x] Multiple accounts in one unified view. Settings > Accounts adds (the browser sign-in asks which account) and signs out accounts. The inbox, split tabs and counts span every signed-in account's inbox (`Scope::Inboxes`); search spans all mail. With two or more accounts each row gets an account-coloured dot, the composer a From picker (replies and forwards go from the account the mail came to), and `account:` (alias `in:`) filters by account in search and splits. Conversations never merge across accounts. Calendar already showed every account's events.
- [x] Windows notifications, start with Windows, tray icon. Toasts for new unread inbox mail (focused only by default; all or off in Settings), only while PeetMail isn't the active window; more than three at once become one summary; snoozes coming back raise a reminder. A per-account watermark on the local row id means the first sync, a backlog and mail older than a day never notify. Tray icon (click opens; menu: Open, Sync now, Quit); closing the window hides to the tray (setting); `Ctrl+Q` quits. Start with Windows (HKCU Run key via tauri-plugin-autostart) launches hidden with `--minimized`. Single instance outside tests: a second launch shows the running window. Clicking a toast doesn't open the conversation (the notification plugin has no click callback on Windows).
- Gmail provider (in progress, branch `gmail`):
  - [x] Slice 1: Google sign-in (PKCE, loopback, optional client secret), Gmail sync (inbox + sent in the cache window, then the History API; a full resync when history has expired), labels mapped to folders (TRASH, our snooze label, INBOX, SENT, else archive; promotions/social/updates/forums are Other), bodies and attachments from the MIME tree (inline cid images, charsets), triage as label changes (archive, read, star, trash, snooze to a hidden `PeetMail/Snoozed` label, undo).
  - [x] Slice 2: send, reply, reply all, forward. PeetMail builds the RFC 2822 message (encoded headers, plain text + HTML, multipart/related for the quoted original's inline images, multipart/mixed for forwarded attachments); replies carry In-Reply-To/References and Gmail's threadId, forwards start a new thread; messages over 4 MB go through the upload endpoint. Undo send and scheduled send work as for Microsoft (same outbox).
  - [x] Slice 3: Google Calendar (primary calendar). Windowed first sync with singleEvents=true, then the nextSyncToken (410: full resync); recurring masters stored but not listed, instances shown. RSVP patches your attendee entry with sendUpdates=all; quick add creates events through the outbox; Gmail invitations get the RSVP card via the UID in their text/calendar part (looked up with iCalUID, cached per message in `message_invites`). Accounts that signed in before calendar access was requested get a "sign in again" message.

## Your own Entra or Google app

Release builds sign in with PeetMail's own apps. Registering your own (for local builds, which have none built in, or when an organisation blocks PeetMail's) is described for users in [docs/user/your-own-app.md](user/your-own-app.md), which the app also shows under Help.

## Risks and open questions

- **Delta and the date filter.** It's unverified whether Graph accepts a `receivedDateTime ge` filter on message delta for both M365 and Outlook.com accounts. If it returns 400, sync falls back to fetching everything and filtering locally, which is correct but slower on the first sync. Check the logs on first sync.
- **WebView2 memory.** Expect roughly 100 to 150 MB. Watch it as features land, and keep the DOM small (the virtual list, collapsed messages).
- **Single DB mutex.** Fine while queries take microseconds. If sync writes ever cause visible UI stalls, split into a writer connection and a read pool.
- **Calendar scope.** Outlook-style calendars get feature-heavy fast. Hold the line on minimal.

## Event details and editing (after Phase 3)

- [x] Details popup: description (sanitized, in a sandboxed frame that never scrolls itself), repeat pattern in words, a banner on occurrences, calendar, location, join link (also found in the location or description: Teams, Zoom, Meet, Webex...), show-as, reminder, private, categories, organiser and guests grouped by answer (collapsed past 5), RSVP with this-occurrence/series scope. Join buttons on calendar blocks.
- [x] Editor (create and edit): title, calendar, all day, start/end (length kept when the start moves), location, guests with suggestions, show-as, reminder, private, rich-text description (Tiptap). Only changed fields are sent (Graph PATCH / Google PATCH with sendUpdates=all). Recurring events: edit this occurrence or the whole series (the master), shown in a banner. Guests can change only their show-as and reminder. Delete: your own event, cancel a meeting you organise (guests told, optional message), or remove an invitation from your calendar; occurrence or series.
- [x] Rooms (Microsoft 365): the location suggests the tenant's rooms (beta `/me/findRooms`, which needs only User.Read; v1.0 Places needs admin-consented Place.Read.All and is only a fallback). Picking one sets the location (`locationType: conferenceRoom` + its mailbox) and invites the room as a `resource` guest, which books it; choosing another swaps it, and typing over the location or removing the room chip un-books it. Personal accounts and Google calendars get no suggestions. Cached per account for 6 hours. Some tenants refuse findRooms with plain User.Read (403 Access is denied): the editor then offers to grant User.ReadBasic.All (user-consentable) by reconnecting the account, and if that's still refused, Place.Read.All (admin approval) for the Places API. Each account stores the scopes it signed in with (`TokenSet.scopes`), because Entra re-asks for them on every refresh and fails on any never granted. A refused lookup logs the token's `scp` claim.
- [x] Reconnect (Settings > Accounts, and the room prompt): signs a known account in again in place (login_hint, same-account check), keeping its mail and settings; also the way back for a signed-out account.
- [x] Repeat editor: presets from the start date (daily, weekdays, weekly, monthly on the nth/last weekday or the date, yearly) that follow it when it moves, or custom (every N days/weeks/months/years, weekdays, monthly mode), ending never / on a date / after N times. Editable when creating, on your own single events (which become series) and on whole series (stopping one warns that the other occurrences go); read-only on one occurrence and as a guest. A series is sent in local wall-clock time with its IANA zone (`Times.local_start`), so it keeps its time across daylight saving; single events stay in UTC.


## Folders (after Phase 3)

Views of the well-known folders, unified across accounts like the inbox (and filtered by the footer account toggles). Custom Outlook folders and Gmail labels are out of scope for now; their mail is still in All mail and search.

| View | Keys | Holds |
|---|---|---|
| Inbox | `g i` | As now, with splits (splits only ever apply to the inbox) |
| Done | `g e` | Archive (Gmail: no INBOX/SENT/TRASH/SPAM label) |
| Sent | `g t` | Sent Items |
| Snoozed | `g h` | Snoozed, each row with its "back at" time |
| Scheduled | `g s` | Waiting sends (outbox rows, not cached messages); `Enter` edits in the composer, `#` cancels |
| All mail | `g a` | Every cached folder except Trash, Done included |
| Trash | `g #` | Deleted Items |

Slices:

1. [x] **Sync Archive and Deleted Items** in the mail window. Microsoft: add both to `SYNCED_FOLDERS` (one more delta request per folder per account per minute when idle). Gmail: an initial `-in:inbox -in:sent -in:trash -in:spam after:` listing plus `TRASH`, and drop `only_known_archive` so History label changes fill Done in. Prune by the window like the inbox. First syncs after the update take longer (a busy archive is thousands of messages); notifications stay inbox-only, so none fire. Archive is optional on Microsoft (a 404 skips it); `wk:` placeholder folders from earlier local moves get their real id in place. Gmail accounts synced before this list archive and trash once (`gmail_all_folders:<id>`).
2. [x] **Scope by role.** `db::Mailbox` (inbox, done, sent, snoozed, all, trash) maps to a `Scope`: `Role(role)` over signed-in accounts, `Done` (archived conversations with nothing in the inbox or snoozed, so a reply arriving in a done conversation moves it back), `AllMail` (signed-in accounts, excluding Trash; search uses it too). `list_threads` takes `mailbox`; `get_thread` takes `withTrash` for the Trash view's reader.
3. [x] **UI.** `mail.mailbox` beside `mail.view`; the Rail gains Inbox, Done, Sent, Snoozed, Scheduled, All mail, Trash (icons, tooltips with keys, unread dot on Inbox); split tabs only in the Inbox, a folder title elsewhere; `g` bindings in the keymap registry. Triage follows the folder: in Done `e` is "not done" (back to the inbox, undoable); in Trash `#` restores to the inbox; in Sent and All mail `e`/`#` act on the conversation's inbox messages as now.
4. [x] **Snoozed and Scheduled become list views**, replacing their dialogs (`h` change time and `u` back now keep working in Snoozed).
5. [x] Fake Graph/Gmail serve an archive and trash; E2E for each view and its triage; help page shortcut table; changelog.

## Roadmap candidates

Gaps found by reviewing the app against what people expect from a keyboard-first Superhuman/Outlook-style client (October 2026). None are committed to; each still has to earn its place (principle 2). Tiers are rough priority. Key suggestions follow Superhuman where the key is free; clashes with existing bindings are noted.

### Tier 1: blockers for daily use

Without these, people go back to Outlook or Gmail for routine tasks.

- [x] **Attachments when composing.** File picker (`ctrl+shift+a`, paperclip), drag and drop (the window's native drop handler is off so the page gets HTML5 drops), and paste (files only when the clipboard has no text, so Word/Excel copies stay text). Each file's bytes go to Rust as a raw IPC body (`add_draft_file`) into `draft_files` (migration 0011), and the draft keeps `attachments: [DraftFile]`; a scheduled send has the bytes even if the file moves. Files are dropped once the send goes or fails for good, when the composer is closed or replaced, and at startup if no waiting send refers to them. 25 MB in all (Gmail's and Outlook.com's limit). Forwards list the original's attachments; removing one records its name in `droppedAttachments`. Graph: a message with files goes as a draft (`/me/messages`, `createReply`/`createReplyAll`/`createForward`), forwarded attachments removed by name, files under 3 MB as `fileAttachment`s and larger ones through an upload session (3.125 MB chunks, no bearer token), then `/send`; a failure part-way deletes the draft. Messages without file changes keep the single call. Gmail: the files join the multipart/mixed part, dropped forwards left out.
- [x] **Bcc.** `Draft.bcc`; a field and button beside Cc, `ctrl+shift+b` (`compose.bcc`), with contact suggestions. Graph: `bccRecipients` on sendMail and in `message` for reply/replyAll/forward (forwards now carry Cc too, which they silently dropped). Gmail: a `Bcc:` header, which Gmail removes on delivery. A message may go to Bcc alone.
- **Drafts.**
  - [x] Step 1, local: the composer saves itself as you type (debounced 800ms) into `drafts` (migration 0012; the composer's state as JSON, `AUTOINCREMENT` ids so a late save can't land on a newer draft, a `seq` for last-edited order). Esc closes and keeps it (one press; the double-Esc discard is gone), and so does opening another composer (`MailStore.composer`'s setter puts the old one away); an empty one (nothing typed or attached; a reply's own To and subject don't count, `worthKeeping`) leaves nothing. Ctrl+Shift+D or the bin discards, with its files. Sending removes the draft in the same transaction as queueing the send, and no save may run once a send has started (`#sending`, and saves are serialized in `#draftOps`). A draft's files survive the startup purge. Drafts view (`g d`, rail): Enter reopens (a reply under its conversation if that's showing), # discards. Survives a crash (E2E kills the process).
  - [x] Step 2, synced with the server, last save wins (`draft_sync.rs`). A local save marks the draft `dirty` and queues one `Op::PushDraft` per draft (3s, read when it runs). Graph: created with `POST /me/messages`, or `createReply`/`createReplyAll`/`createForward` so Outlook threads it, then PATCHed; attachments matched by name (a forward keeps the originals Graph copied in, minus the ones taken out); version = `lastModifiedDateTime`. Gmail: `drafts.create`/`update` with the full MIME (threadId and In-Reply-To/References for replies; the upload endpoint over 4 MB); version = the draft's message id. Each sync lists the server's drafts (one request when nothing changed) and fetches only versions that moved on; a draft changed here keeps its edits, otherwise the server's copy replaces it, and one gone from the server goes here (unless it's being sent). Our own pushes come back with the version we stored, so they aren't re-imported. `drafts_lock` stops a pull racing a push's create; `transient_drafts` hides graph_send's in-passing drafts. Sending hides the draft row (`hidden`) instead of deleting it: undo send brings it back, a permanent failure returns it to Drafts, success deletes it and the server copy. Drafts started elsewhere (`imported`) are sent through their server copy (Graph `/send`, Gmail `drafts.send`), keeping their threading. Closing a draft unchanged doesn't save (so it can't overwrite a newer edit from elsewhere). Imported HTML goes through `sanitize::import_draft_html` (editor markup only; text-only `<div>`s become lines).
  - [x] Drafts in the Inbox, like Superhuman: new-message drafts are rows of the Inbox's own tab (not splits, search or other mailboxes), merged by time (`withDraftRows`; ones older than the loaded page wait for the next), marked Draft; Enter opens, # discards, other triage keys explain. A reply or forward draft (also one written elsewhere whose conversation is cached: Graph conversationId / Gmail threadId) marks its conversation's row and shows as a card under the messages; r/a (a reply) or f (a forward) carries on with it instead of starting another. A reply stays with its conversation: leaving the conversation puts it away quietly (`followScreen`, run by an effect), coming back shows it inline again without focus; an inline composer only takes the keyboard while focus is in it (`composerActive`), so the reader's keys work around it. Closed with Esc while its conversation shows, it stays a card there until you leave.
- [x] **Signatures.** One per account, made in Settings > Mail with the Tiptap editor (bold, italic, underline, lists, links) and stored as HTML cleaned to the composer's markup in the `signatures` setting, keyed by lowercase address. Keying by address means a signature survives sign-out and re-add, and send-as addresses can get their own later. Always added to new messages; added to replies and forwards when chosen. In the composer it's part of the message's rich text: a `Signature` node (`signatureNode.ts`, isolating, so Backspace doesn't merge it into your text) rendered as `<div class="peetmail_signature">` at the end. Drafts, undo send, Scheduled and server drafts carry it with no extra fields. `worthKeeping` ignores it, so an untouched message with a signature leaves no draft. `Ctrl+Shift+S` removes or restores it, and switching From swaps it unless it was removed. The caret starts above it. `compose_rich` keeps that div (and no other) and marks it `id="Signature"` like Outlook's, a line below the text. `docToText` sets it off with a blank line and `-- `. Not done: images (the editor has no image node), and pasted Outlook signatures lose colours and fonts. A draft started in Outlook or Gmail comes in with its signature as plain lines (`import_draft_html` drops the div), so `Ctrl+Shift+S` there adds a second one.
  - **CodeTwo and similar server-side signatures** (researched October 2026 from CodeTwo's docs). In CodeTwo's cloud mode, an Exchange Online transport rule routes mail from the tenant's users through CodeTwo's Azure service, which adds the signature. That applies to any client, so Graph `sendMail`/`reply`/`forward` should be signed with no work from PeetMail. CodeTwo doesn't document Graph specifically; confirm with a real send from a CodeTwo tenant. Recipients get two signatures if the client adds one too, so each account has "My organisation adds my signature": PeetMail adds none and the composer says why. CodeTwo puts the signature under the latest reply by finding a reply separator from an unpublished list, falling back to the bottom. Graph's own reply markup is Outlook's, so placement should work. Don't put `<hr>` or layout tables in the composed text, which CodeTwo can mistake for a separator. Processed mail carries `X-CodeTwoProcessed: true`, which could let PeetMail suggest the setting (only on mail from colleagues, or in Sent Items when the admin enabled Sent Items Update). There's no API to fetch a CodeTwo or Outlook roaming signature: CodeTwo has no public API, and Microsoft has said Graph won't support roaming signatures. CodeTwo for Google Workspace hadn't shipped as of mid-2024.
- [x] **Formatted text in the composer.** Tiptap (StarterKit, no trailing node) in place of the textarea. Each paragraph is a line (Enter starts the next, an empty one is a blank line), as in Gmail's and Outlook's composers; Rust sends them as `<div>`s (`sanitize::compose_rich`, a strict allowlist: the editor's own markup, http/https/mailto links), so spacing is the same in every mail client. The plain-text part comes from `richtext.ts` (`docToText`: lists as `- `/`1. `, quotes as `> `, links as `text <url>`), also exposed as the editor's `data-text`. Ctrl+B/I/U, Ctrl+Shift+8/7 for lists, Ctrl+K in the text for a link (the palette everywhere else), Markdown shortcuts as you type, and a toolbar in the footer. In the text, Ctrl chords go to the keymap first (so Ctrl+Enter sends rather than adding a line break, Ctrl+Shift+B shows Bcc rather than quoting), the rest to the editor. Drafts queued before this have no `html` and go out as before. Not done: pasted images go inline (they're attached instead).
- [x] **Acting on several conversations at once.** `x` selects, `Shift+J`/`Shift+K` extend, `ctrl+a` selects everything in the view; triage keys (`e` `#` `u` `s` `h`) then act on the whole selection, and one `z` undoes all of it. This is also how people clear a whole view in one go (select all, then `e`). The selection is a set of conversation keys in the store (`selection.ts`), apart from the cursor, and only the list acts on it: opening a conversation, changing view or searching clears it. Ctrl+A loads the rest of the list first. Rust applies a batch in one transaction (`threads_action`, `snooze_threads`), and the undo stack holds steps (`UndoGroup`), each one action or a batch. `u` on a mixed selection reads it all, and `s` flags it all, as Gmail does. Ctrl+click and Shift+click select with the mouse. In the preview layout the reading pane sums up the selection instead of showing a conversation (and doesn't mark the cursor's one read). Outbox ids are `AUTOINCREMENT` (migration 15): undo cancels ops by id, and reused rowids let it cancel a newer, unrelated op.
- [x] **Spam / junk.** A `junkemail` folder role: Graph syncs Junk Email (optional, a 404 skips it); Gmail's SPAM label maps to it (TRASH still wins), with a `labelIds=SPAM` listing at full sync and a one-time backfill (`gmail_spam:<id>`) for accounts synced before. `!` is Spam (`Action::Spam`: inbox, snoozed, archived and deleted mail to Junk; Graph moves to the well-known `junkemail`, Gmail adds SPAM and drops INBOX and the snooze label), and in the Junk view (`g !`) `!` and `e` are Not spam (back to the inbox, SPAM off); `#` there deletes, and its undo goes back to Junk. Junk, like Trash, stays out of All mail, search, Recent Opens, contact suggestions and other views' readers (`HIDDEN_ROLES`; `get_thread` takes the view it's opened from). Not done: Graph's beta `markAsJunk` (which also blocks the sender) and the Gmail-only "report phishing".
- [x] **Searching the server.** Local search covers only the cache window (90 days by default). Under the results (once all are loaded), or with `ctrl+Enter` (`search.server`), PeetMail searches every signed-in account the query covers (`server_search.rs`). `query.rs` translates the query: Graph `$search` KQL (`from:` `recipients:` `subject:` `hasAttachments:true`, words, phrases, `AND NOT`) and Gmail `q` (adds `is:unread|read|starred`, `{to: cc:}`); terms a server can't search (`is:focused|other|invite`, `account:`) are checked locally and `account:` picks the accounts. The newest 100 matches per account. Results are stored like synced mail, in their real folders (Graph: unsynced folders count as archive, Deleted Items and drafts are skipped; Gmail: by labels, spam and trash skipped), so the reader, bodies, triage and replies work unchanged. `search_cache` (migration 0016) marks them as results (`hit`) and as `added` when they weren't cached, and keeps them out of both prunes while the search is open; `list_threads` with `withServer` ORs the hits into the local matches. Opening a result fetches the rest of its conversation (Graph `$filter=conversationId eq`, Gmail `threads.get`). Changing the search text goes back to local results; closing the search (and starting the app) drops what was added, except snoozed mail.

### Tier 2: Superhuman-style power features

- **Remind me if nobody replies.** On send or from a conversation: if no reply has arrived by a time, the conversation comes back to the inbox. Reuses the snooze machinery (wake time, Reminder badge). Noted as not done under Snooze.
- **Move to folder / label.** Custom Outlook folders and Gmail labels are out of scope today, so mail can't be filed and filed mail can only be found through All mail or search. A fuzzy folder/label picker (`l` for label; Superhuman's `v` for move is taken by the reading pane toggle) plus `g l` to open any folder.
- **Unsubscribe** with one key, from the `List-Unsubscribe` header (one-click POST per RFC 8058, else mailto or the link).
- **Mute a conversation** (new replies skip the inbox) and **block a sender**. `m` is free in mail views (it's Maybe only in invites).
- **Moving through messages inside a conversation.** Step between messages (`n`/`p`), expand/collapse one or all (`o`/`Shift+O`), and reply to the focused message rather than the latest. Quote the selected text in a reply.
- **Opening links from the keyboard.** A hint mode that labels the links in the open message, or a palette list of them.
- **Where to go after archiving:** next, previous, or back to the list (Settings > Mail). Today it always moves to the next conversation.
- **Sender details in the right pane.** The sender's name, address and your recent conversations with them, beside or instead of Recent Opens. Later, a contacts list beyond the suggestions taken from mail history.
- **Mark everything read** in a view or split, and "archive everything older than…".
- **Copy a link / open in Outlook or Gmail**, to hand off to the web client for anything PeetMail doesn't do.
- **Clicking a notification opens the conversation.** The notification plugin has no click callback on Windows; toast activation through the `windows` crate or a `peetmail://` URL scheme could provide one.
- **Editable keyboard shortcuts**, stored as overrides on the keymap registry ids.

### Tier 3: Outlook and workplace features

- **Out-of-office replies** (Graph `automaticRepliesSetting`, the Gmail vacation responder), with a footer indicator while on.
- **Categories and importance**, shown on rows and settable from the keyboard; `categories:` and `is:important` in the query language.
- **Shared mailboxes and send-as aliases** (M365 delegates, Gmail send-as addresses) in the From picker. Their configuration syncs between devices (`docs/SYNC.md`, stage 5).
- **Rules.** Show server inbox rules, and "always move mail from this sender to…" from a conversation.
- **Print / save as PDF** for a conversation.
- **Find in message** (`ctrl+f`) and zoom for message bodies.
- **Composer in its own window**, to write while reading other mail.

### Calendar

- Month view, and a full-screen day view alongside the week view.
- Google calendars beyond the primary one, and turning individual calendars on and off.
- Free/busy when inviting people (Graph `getSchedule`, Google `freeBusy`).
- Event time zones, and a second time-zone column.
- "Schedule a meeting" from a conversation, with its people as guests and its subject as the title.

### General

- **Cloud sync** of settings, signatures, snippets, splits and the account list between devices, end-to-end encrypted through a self-hosted server. Planned in `docs/SYNC.md`.
- A light theme that follows Windows' setting. `theme.test.ts` contrast checks would need a light palette too.
- Density options (compact rows).
- An unread-only toggle per view (today only `is:unread` in search or a split).
