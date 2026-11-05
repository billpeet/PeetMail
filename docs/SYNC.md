# Cloud sync plan

Settings, signatures, snippets, splits, the list of accounts and (later) shared mailboxes follow you between devices, through a small server you host yourself. The server stores only ciphertext. Work is split into stages; each one ships on its own and leaves `pnpm verify` green.

## Status

| Stage | State |
|---|---|
| 1 Item-shaped settings | Done |
| 2 Sync server | Done (image built in CI, not yet published) |
| 3 Client sync and Settings → Sync | Done (Microsoft accounts tested end to end; Google untested, see Risks) |
| 4 Synced account list | Not started |
| 5 Shared mailboxes and send-as | Not started (feature itself is new, see PLAN.md Tier 3) |

## Decisions

| Question | Decision |
|---|---|
| Who can read the data | Nobody but your devices. End-to-end encrypted; the server sees opaque item IDs and ciphertext. |
| Sign-in | With a mail account already in PeetMail: the server verifies its OpenID `id_token`. No passwords, no email sending on the server. A sync account can have several mail accounts linked (see [Several mail accounts](#several-mail-accounts)). |
| Server stack | SvelteKit (adapter-node) + Drizzle + SQLite, in `server/` as a pnpm workspace package. Leaves room for a web client later. |
| Hosting | Self-hosted only: a Docker image and a plain `node build` both work. No Peet Software instance for now. |
| Default | Off. Settings → Sync takes a server URL. |
| Tokens | Never synced. Each device signs in to each mail account itself. |
| Merge model | Per item, last write wins by a hybrid logical clock inside the encrypted blob. The server only does compare-and-swap on a version number. |
| Egress | Only the Rust core talks to the sync server, like Graph, Sentry and the updater. The webview's CSP stays `connect-src ipc:`. |

## What syncs

| Syncs | Stays on the device |
|---|---|
| Splits, hide empty splits, signatures, snippets, undo-send delay, week start, notifications | Pane and list widths, layout (depends on the screen), close to tray |
| The list of accounts: provider, address, which OAuth app (stage 4) | Tokens, cache window, body cache size |
| | Update channel, install ID, changelog seen, privacy notice seen, restart-to-update |
| Later: shared mailboxes and send-as per account, "my organisation adds my signature", editable shortcuts | Crash reports and usage data consent (a new machine asks for itself) |
| | Sync cursors and per-account bookkeeping (`history_key`, `all_folders_key`, snooze label keys) |
| | The user's own Microsoft client ID and Google client ID/secret, for now: changing them signs out the accounts that use them, so a change arriving from another device would sign accounts out unannounced. They can sync once that's handled (they'd be safe to send, encrypted). |
| | Settings sync's own state: the server, the sealed device token and key, the cursor, the clock's node id, which accounts are linked |

`sync/mod.rs` lists every settings key (or key prefix) as synced or local. A key in neither list is a mistake: `db::set_setting` asserts against it in debug builds (so any test that writes it fails), and a unit test scans the source for `KEY_*` constants, literal keys passed to the settings functions and `format!` key builders. Each new setting gets a deliberate choice.

## Data model

### Items, not blobs

Today `splits`, `snippets` and `signatures` are each one JSON value. With last-write-wins on the whole value, editing one snippet on the laptop and another on the desktop loses one edit. Stage 1 stores them as one setting per item:

- `signature:<lowercase address>` → `{html, replies, managed}`
- `snippet:<name, ASCII-lowercased>` → `{name, body, order}`. Names are already unique ignoring case, so the same snippet made on two devices lands on one key, and no ids are needed.
- `split:<random id>` → `{name, query, order}`. The id travels to the UI and back (`Split.id`), so renames keep it. A new install's default Other tab has the fixed id `split:other`, so devices that sync later share one instead of each adding theirs.
- Scalars stay as they are (`undo_send_secs`, `week_starts_on`, …).

`order` is a number between the neighbours' (`items::assign_orders`): a save keeps the order of the longest run of entries still in sequence and renumbers only the ones that moved, falling back to 1, 2, 3… when floating-point gaps run out. Saving writes only entries whose value changed and deletes removed ones (`items::replace`), so a later sync pushes only real edits.

Migration `0014_settings_items.sql` moves the old lists over in SQL, treating unreadable values as empty rather than failing the start-up, and folds in the old one-off "drop the Important default split" migration. The UI reads and writes the same shapes as before through the Rust commands, so this stage changes nothing visible.

Diagnostics (`support.rs`) show settings keys containing `:` as a count per prefix (`signature:*: 2`), since the rest of such a key can be an address.

### On the client

Migration `0016_settings_sync.sql` adds:

```sql
CREATE TABLE sync_items (
  key        TEXT PRIMARY KEY,   -- the settings key, e.g. 'snippet:thanks'
  hlc        TEXT NOT NULL,      -- hybrid logical clock of the local value
  server_seq INTEGER,            -- version the server last confirmed; NULL = never in step
  dirty      INTEGER NOT NULL DEFAULT 0,
  deleted    INTEGER NOT NULL DEFAULT 0
);
```

- **Tracking local changes** (`sync/store.rs`). `db::set_setting` and `db::delete_setting` stamp a synced key and mark it dirty, whether or not sync is on, so turning it on later knows what was really edited. Writing a value a setting already has changes nothing: `set_preferences` rewrites every preference on each change, and stamping unchanged ones would let a stale value beat a real edit made elsewhere. Each change also wakes the sync loop.
- **The clock** (`sync/hlc.rs`): `<ms, 13 digits>:<counter, 4 digits>:<node>`, compared as strings. The next stamp passes both the PC's time and the newest stamp seen, so an edit made after syncing wins even on a PC whose clock is behind. Stamps more than a day ahead are refused, so one PC with a wrong clock can't freeze a setting. Never-edited settings (defaults) carry the zero stamp and lose to anything.
- **Applying another device's change** writes the setting without stamping it (`db::write_setting_unsynced`), records the remote stamp and version, then emits `settings-changed {keys}`. The UI reloads preferences, snippets, signatures and splits (`mail.reloadSettings`). Only keys classified as synced are ever applied: the server can't change this device's cache size or sync state.
- **A run** (`sync/engine.rs`): pull every page since the cursor and merge; push dirty items in batches of 200, each with the version it was made on; merge each conflict and push again (three rounds at most, the rest waits for the next run); then link any of this PC's accounts that aren't linked. Runs a few seconds after startup, every sync interval (60 s), two seconds after a local change, and on window focus. "Sync now" runs it at once.
- **Item plaintext**: `{v: 1, key, value, hlc}`, `value` null for a removal. A device skips (and logs) an item it can't decrypt, one whose key doesn't match its id, one from a newer `v`, or one with an implausible stamp; one bad item doesn't stop the rest.
- **The device's secret** (its device id, token and the master key) is sealed with `secure.rs` (DPAPI on Windows, the keyring key on Linux), like sign-in tokens, in the local setting `settings_sync_secret`.
- **A device the server signed out** (`401 device_revoked`/`unauthorized`) turns sync off by itself and says why in Settings → Sync.

### On the server

```
users       (id, created_at, wrapped_key, kdf_params, seq)          -- seq: the last version handed out
identities  (id, user_id, kind, subject, email_hint, linked_at)   UNIQUE (kind, subject)
devices     (id, user_id, name, token_hash, created_at, last_seen_at, revoked_at)
items       (user_id, item_id, seq, blob, deleted, updated_at)    PRIMARY KEY (user_id, item_id)
```

Drizzle schema in `server/src/lib/server/schema.ts`, migrations in `server/drizzle/` (applied at startup).

`email_hint` is shown only in the server's own logs and device list; it is the one piece of personal data the server keeps, because a self-hoster needs to know whose account is whose.

## Encryption

- A random 256-bit **master key** per sync account, made by the first device.
- The passphrase goes through Argon2id (parameters stored with the user, so they can be raised later) into a key-encryption key that wraps the master key. The wrapped key lives on the server; a new device downloads it after sign-in and unwraps it with the passphrase.
- Setup also shows a **recovery key** (the master key as a printable code). Losing both is low stakes: every device has the full data and can upload again under a new passphrase.
- Item blobs: AES-256-GCM, random 96-bit nonce, the item ID as associated data (so the server can't swap blobs between items). Plaintext is `{v: schema, hlc, value}`. AES-GCM rather than ChaCha20 because WebCrypto has it natively, which a web client will need. `ring` does both.
- Item IDs: HMAC-SHA256(master key, settings key), base64url. The server never sees `signature:bill@example.com`.
- The master key is kept locally through `secure.rs` (DPAPI on Windows, the keyring-held key on Linux), like tokens.
- Changing the passphrase rewraps the master key; no items change. Rotating the master key (after a lost device) re-encrypts every item; rare enough to be a button in Settings, not automatic.

## Protocol

JSON (camelCase) over HTTPS, `Authorization: Bearer <device token>` on everything except health and sign-in. Types and request checks: `server/src/lib/protocol.ts`; one example of each body: `server/protocol-fixtures/examples.json`.

| Call | Does |
|---|---|
| `GET /v1/health` | `{ok, protocol, version}`, so PeetMail can check a URL before signing in. |
| `POST /v1/session` `{provider, idToken, deviceName}` | Verifies the token, finds the user by identity, signs a new device in. Returns `{deviceId, deviceToken, wrappedKey, kdfParams}`, or `404 no_user` if the identity isn't linked (never a new user). |
| `POST /v1/users` `{provider, idToken, deviceName, wrappedKey, kdfParams}` | Creates a sync account with this identity and signs the device in. `403 registration_closed` unless `ALLOW_REGISTER` lets the identity in; `409 already_registered` if it's already linked. |
| `GET /v1/identities` | The linked mail accounts: `[{id, provider, email, linkedAt}]`. |
| `POST /v1/identities` `{provider, idToken}` | Links another mail account (harmless if already linked to this user). `409 linked_elsewhere` if it belongs to another user. |
| `DELETE /v1/identities/:id` | Unlinks; `409 last_identity` for the last one. |
| `GET /v1/items?since=<seq>&limit=<n>` | Items with `seq > since`, oldest first, at most 1000 a page; tombstones included. `{items: [{id, seq, blob, deleted}], cursor, more}`. |
| `PUT /v1/items` `{items: [{id, blob, deleted, baseSeq}]}` | Compare-and-swap per item, in one transaction: writes when the stored `seq` equals `baseSeq` (or there's no stored item), assigning the next `seq`. An item that exists but was pushed with `baseSeq: null` conflicts too. Each returns `{id, status: "ok", seq}` or `{id, status: "conflict", seq, blob, deleted}`. At most 500 items a push and 10,000 per user (`413 too_many_items`). |
| `GET /v1/devices`, `DELETE /v1/devices/:id` | List (`[{id, name, createdAt, lastSeenAt, current}]`) and revoke devices. Revoking your own signs it out of sync; a revoked device's token gets `401 device_revoked`. |
| `PUT /v1/key` `{wrappedKey, kdfParams}` | Passphrase change. |
| `DELETE /v1/users/me` | Deletes the sync account and everything in it. |

Errors are `{error, message}`: a stable code for the client and a sentence it can show. Device tokens are `pmd_` plus 32 random bytes, stored as SHA-256 hashes.

On a conflict the client decrypts the server's copy, keeps whichever has the later clock, and pushes again with the new `baseSeq` if its own value won. The server never merges.

Older clients keep items they don't understand (an unknown key or a newer `v`) and never delete them. A newer schema for an existing key means a new key name (`snippet2:<id>`), so old clients keep working with the old one.

Sync runs in the existing 60s poll, on window focus, and 2s after a local change. Server-sent events can come later if that feels slow.

## Several mail accounts

The sync account belongs to a person, not to a mail account. Any number of mail accounts can be linked to it, and any linked one signs a device in.

- **Identity key.** Microsoft: `tid` + `oid` from the id_token, not `sub`. Entra's `sub` differs per app, so the same person signing in through PeetMail's app and through their own custom app would otherwise look like two people. Google: `sub`, which is the same across apps.
- **Setting up.** Settings → Sync → Turn on. PeetMail tries each of this device's accounts against `/v1/session`. If one is linked, the device joins that sync account (and asks for the passphrase). If none is, it offers to create one, with a picker for which account to start with (default: the first). A sync account is never created without that explicit step, so a new device signed in to a different account than before can't silently start a second, empty one.
- **Linking more.** While sync is on, PeetMail links the device's other accounts automatically, and any account added later. The device holds a fresh id_token for each of them anyway (from sign-in, or from a token refresh, which returns one when `openid` is in the scopes). Settings → Sync lists linked accounts with an unlink button.
- **Already linked elsewhere.** Auto-linking skips an account that belongs to another sync account and says so in Settings → Sync. Merging two sync accounts is out of scope for now.
- **Removing a mail account** from PeetMail doesn't unlink it; it can still sign a future device in. Unlinking is a separate action.
- **Losing a mail account** (leaving a job): device tokens don't depend on the mail accounts staying valid, so any device already in sync keeps working and can link another account. Settings → Sync suggests linking a personal account when every linked account is from one organisation.
- **Refreshing an id_token.** The server accepts id_tokens up to 10 minutes old (`iat`, with a minute's clock tolerance), RS256-signed with the provider's published keys, for the configured audiences. Microsoft's issuer must be its tenant's own (`.../{tid}/v2.0`). An id_token proves identity only; the data stays safe behind the passphrase even if one leaks.
- **Which address counts.** Microsoft: `preferred_username` (the UPN for work accounts, whose domain the tenant verified; the address for personal ones). Entra's `email` claim is unverified and never used. Google: `email`, only when `email_verified`. This is the address `ALLOW_REGISTER` matches and the server keeps as `email_hint`.

## Self-hosting

`server/` builds a Node app (`node build`) and a Docker image (`server/Dockerfile`, built from the repository root; CI builds it, nothing publishes it yet). All the settings are in [server/README.md](../server/README.md); the ones that shape the design:

| Variable | Meaning |
|---|---|
| `ALLOW_REGISTER` | Who may create a sync account: addresses, `@domain`, `tenant:<tid>`, `*` or `none`. Required: the server won't start without it, since it's reachable from the internet. Linking more accounts to an existing user is always allowed. |
| `MICROSOFT_CLIENT_IDS`, `GOOGLE_CLIENT_IDS` | Accepted id_token audiences. Microsoft defaults to PeetMail's built-in app; Google is off until set (PeetMail's Google app isn't registered yet). |
| `MICROSOFT_DISCOVERY_URL`, `GOOGLE_DISCOVERY_URL` | The OpenID configuration documents. Overridable so tests (and stage 3's E2E harness) can point the server at a local issuer. |

HTTPS is the reverse proxy's job; the client will refuse plain http except to localhost. Backups are the SQLite file. User help goes in `docs/user/sync.md` (stage 3); running the server is `server/README.md`.

## Repo layout

- `pnpm-workspace.yaml` with the root app and `server/`.
- `server/`: SvelteKit 3 (adapter-node), Drizzle 0.45 with `better-sqlite3` (prebuilt binaries for every platform, so pnpm is told not to compile it), `jose` for id_tokens, Tailwind for its one page. `src/lib/server/service.ts` holds every call's logic independent of HTTP; `src/routes/v1/` are thin handlers. `pnpm server:check` and `pnpm server:test` are part of the root `pnpm verify`, and CI's `server` job also builds the Docker image.
- Protocol types are hand-written on both sides (`server/src/lib/protocol.ts`, `src-tauri/src/sync/protocol.rs`), kept honest by shared JSON fixtures in `server/protocol-fixtures/` that a Vitest and a Rust test both read, and by the E2E tests running against the real server.
- Rust: `src-tauri/src/sync/` (`mod.rs` engine, `crypto.rs`, `protocol.rs`, `hlc.rs`).

## Stages

### Stage 1: item-shaped settings

Split `splits`, `snippets` and `signatures` into per-item keys with a migration; add the synced/local registry and its checks; `delete_setting` and `settings_with_prefix`. No visible change, so no changelog line.

**Tests.** Rust: the migration converts each old shape, skips bad entries, gives a new install and an unreadable split list the default Other tab (and keeps an empty list empty), and drops the old Important default only when it hadn't been already; order assignment (appends, inserts, moves, swaps, exhausted gaps, 200 repeated moves); saves write only changed entries; split ids survive cleaning and duplicates get fresh ones; the source scan finds an unclassified key; diagnostics count item keys without showing them. The existing E2E tests for splits, snippets and signatures pass unchanged.

### Stage 2: sync server

`server/` package, schema, the calls above, id_token verification against JWKS (discovery cached for a day), device tokens (random, stored hashed), `ALLOW_REGISTER`, a status page, the Docker image built in CI. Nothing in PeetMail uses it yet, so no changelog line.

**Tests** (Vitest, a fresh in-memory SQLite each):
- Service: compare-and-swap (the current version wins, stale and blind pushes get the server's copy, one conflict doesn't block the rest), tombstones, paging by `seq`, per-user isolation and versions, the item cap leaves nothing half-written, registration rules, sign-in never creates a user, link/unlink rules, revoked and other users' devices, last-seen throttling, passphrase change, account deletion.
- id_tokens, signed by a local issuer (`testing/issuer.ts`, shaped like Microsoft's and Google's): one identity across two apps (tenant + oid), wrong audience, a token claiming another tenant's issuer, the unverified `email` claim ignored, Google's verified address, Google off by default, expired, stale, forged, unknown key, `alg: none`, an unreachable provider (502, not a refusal).
- Routes: a sync account's whole life through the real handlers with real tokens, each response checked against the shape of its example in `protocol-fixtures/`; bad JSON and bad fields get `400` with a message.
- By hand: the built server and the Docker image start, migrate, answer `/v1/health`, and refuse to start without `ALLOW_REGISTER`.

### Stage 3: client sync and Settings → Sync

The engine, crypto, Settings → Sync (server URL, connect, then unlock with the passphrase or recovery key, or create with a new passphrase and see the recovery key once; once on: status, Sync now, linked accounts with unlink, devices with sign out, passphrase change, show recovery key, turn off, delete the sync account), palette entries "Settings sync" and "Sync settings now", `docs/user/sync.md` (Help → Syncing settings), a changelog line.

- **Turning it on** is two steps. `settings_sync_connect` checks the server's `/v1/health` (protocol 1), then tries each signed-in account's fresh id_token against `/v1/session`. The first that's known gives a session, and the UI asks for the passphrase (`settings_sync_join`). If none is known, and at least one could be checked, the UI offers a new sync account (`settings_sync_create`). If no account could be checked at all, the error is shown rather than offering to create one. Cancelling a join revokes the device the session made.
- **Fresh id_tokens** come from a forced token refresh (`AppState::fresh_id_token`, `auth::refresh_with_id_token`), which also saves the rotated tokens.
- **Passphrases** are at least 10 characters. Argon2id runs on a blocking thread (64 MiB, 3 passes).
- **Unlinking** an account marks the matching local account `unlinked`, so auto-linking doesn't put it straight back.
- **Device names** are the PC's name (`COMPUTERNAME`, `/etc/hostname`).
- **A server bug found here:** SvelteKit's CSRF check treated a bodyless `DELETE` or `PUT` without an `Origin` as a cross-site form and refused it (403). That broke revoke, unlink and delete account, but only in a built server, which is why stage 2's route tests (calling handlers directly) missed it. The server now trusts every origin (`csrf.trustedOrigins: ['*']`): nothing it serves uses cookies.

**Tests.**
- Rust:
  - crypto: round trip, associated data stops a swapped blob, tampering, wrong key, wrong passphrase, recovery key round trip, RFC 4648 base32, default cost
  - clock: ordering, a clock going back, beating a slow clock after seeing a fast one, ties, implausible stamps
  - store: only real changes are stamped, tombstones, remote changes aren't local ones, a change made during a push stays dirty, first sync sends everything with defaults at zero
  - engine merge: later wins, earlier loses but still goes up, new settings and removals arrive, untouched defaults take the other device's value, items it mustn't take are skipped
  - the sealed secret, server URL rules (https except for this PC)
  - the protocol types read and write `server/protocol-fixtures/examples.json` unchanged
- Fake Graph issues RS256 id_tokens under a fixed `{tenantid}` issuer template, with discovery and keys at `/oauth/v2.0/.well-known/openid-configuration`, shared by both fakes in the two-mailbox proxy. `tests/harness/sync-server.ts` builds the real server (when its sources change; global setup does it once) and runs one per test.
- E2E (`settings-sync.spec.ts`), two app instances with separate data dirs against the real server:
  - settings made before sync go up
  - B joins with the passphrase and gets A's snippet
  - B's split shows as a tab on A
  - both edit one snippet and the later wins on both
  - device-only settings stay put
  - a wrong passphrase is refused, and the recovery key unlocks (typed loosely)
  - a device signed out from another stops and says so
  - turning off keeps settings, and rejoining asks for the passphrase
  - http to another host is refused, as is a server that isn't a sync server
  - a second account on the PC is linked automatically
  - **canary**: none of the snippet, signature and split strings, nor the `snippet:`/`signature:`/`split:` key names, appear in the server's SQLite file, WAL or log

### Stage 4: synced account list

`account:<provider>:<address>` items. A device that doesn't have a synced account shows it under Settings → Accounts as "Not signed in on this device" with a sign-in button, and the footer suggests it once. Removing an account on one device asks whether to remove it everywhere.

**Tests.** E2E: an account added in A shows in B as not signed in; signing in from there works against the fake server; "remove everywhere" removes it in B.

### Stage 5: shared mailboxes and send-as

The feature from PLAN.md Tier 3, with its per-account configuration stored as synced items from the start (`shared_mailboxes:<address>`, `send_as:<address>`). Planned separately when it starts.

## Risks and open questions

- **Microsoft id_token issuer.** Multi-tenant tokens have issuer `https://login.microsoftonline.com/{tid}/v2.0`; personal accounts use tenant `9188040d-6c67-4c5b-b112-36a304b66dad`. Check with a work and an Outlook.com account before stage 2 relies on it.
- **id_token on refresh.** Entra returns one on refresh with `openid` in the scopes. Google's documentation says a refresh returns one too when `openid` was granted, but this is untested (the fake Gmail doesn't issue id_tokens, and PeetMail's own Google app isn't registered). If a refresh returns none, connecting or linking with that account says "reconnect it under Settings → Accounts"; check with a real Google account before relying on it.
- **Concurrent reorders.** Two devices reordering splits offline can interleave oddly. Fractional indexes keep it from losing items; the order may just need fixing by hand.
- **Error text.** Sync server errors go through `scrub` before reaching Sentry or logs, like Graph errors.
- **Rate limiting.** The server has none. Signing in needs a valid id_token for an accepted app, so abuse is limited to a signed-in device hammering its own account; a reverse proxy can rate-limit if that ever matters.
- **Publishing the image.** CI builds it; pushing to `ghcr.io/billpeet/peetmail-sync` on release is a small release.yml job. Now that the client exists it's worth doing with the first release that ships stage 3.
- **Real providers end to end.** Everything is tested against the fakes. Before announcing it, sign in from two PCs with a real work account and a real Outlook.com account against a real server behind HTTPS.
- **Web client later.** The crypto choices (AES-GCM, Argon2id via WASM) keep it possible; a web client would also need its own OAuth redirect for the id_token, so the server would accept a third audience.
