# Cloud sync plan

Settings, signatures, snippets, splits, the list of accounts and (later) shared mailboxes follow you between devices, through a small server you host yourself. The server stores only ciphertext. Work is split into stages; each one ships on its own and leaves `pnpm verify` green.

## Status

| Stage | State |
|---|---|
| 1 Item-shaped settings | Done |
| 2 Sync server | Not started |
| 3 Client sync and Settings → Sync | Not started |
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
| The user's own Microsoft client ID and Google client ID/secret (safe because encrypted) | Update channel, install ID, changelog seen, privacy notice seen, restart-to-update |
| Later: shared mailboxes and send-as per account, "my organisation adds my signature", editable shortcuts | Crash reports and usage data consent (a new machine asks for itself) |
| | Sync cursors and per-account bookkeeping (`history_key`, `all_folders_key`, snooze label keys) |

`sync.rs` lists every settings key (or key prefix) as synced or local. A key in neither list is a mistake: `db::set_setting` asserts against it in debug builds (so any test that writes it fails), and a unit test scans the source for `KEY_*` constants, literal keys passed to the settings functions and `format!` key builders. Each new setting gets a deliberate choice.

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

Migration `0015_sync.sql`:

```sql
CREATE TABLE sync_items (
  key        TEXT PRIMARY KEY,   -- the settings key, e.g. 'snippet:7f3a'
  hlc        TEXT NOT NULL,      -- hybrid logical clock of the local value
  server_seq INTEGER,            -- version the server last confirmed; NULL = never pushed
  dirty      INTEGER NOT NULL DEFAULT 0,
  deleted    INTEGER NOT NULL DEFAULT 0
);
```

`set_setting` (and a new `delete_setting`) bump the clock and mark the row dirty when the key is synced. Applying a pulled item writes the setting without marking it dirty, then emits `settings-changed {keys}` so the UI stores reload.

### On the server

```
users       (id, created_at, wrapped_key, kdf_params)
identities  (id, user_id, kind, subject, email_hint, linked_at)   UNIQUE (kind, subject)
devices     (id, user_id, name, token_hash, created_at, last_seen_at, revoked_at)
items       (user_id, item_id, seq, blob, deleted, updated_at)    PRIMARY KEY (user_id, item_id)
user_seq    (user_id, seq)                                        -- per-user counter
```

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

JSON over HTTPS, `Authorization: Bearer <device token>` on everything except sign-in.

| Call | Does |
|---|---|
| `POST /v1/session` `{id_token, device_name}` | Verifies the token, finds the user by identity. Returns `{device_token, user, wrapped_key, kdf_params}`, or `404 no_user` if the identity isn't linked. |
| `POST /v1/users` `{id_token, device_name, wrapped_key, kdf_params}` | Creates a sync account with this identity. Only when the identity is allowed to register (see Self-hosting). |
| `POST /v1/identities` `{id_token}` | Links another mail account to the signed-in user. `409 linked_elsewhere` if it belongs to another user. |
| `DELETE /v1/identities/:id` | Unlinks; refused for the last one. |
| `GET /v1/items?since=<seq>` | Items with `seq > since`, oldest first, paged; tombstones included. Returns the new cursor. |
| `PUT /v1/items` `[{id, blob, deleted, base_seq}]` | Compare-and-swap per item: writes when the stored `seq` equals `base_seq` (or the item is new), assigning the next `seq`. Each item returns `ok {seq}` or `conflict {seq, blob}`. |
| `GET /v1/devices`, `DELETE /v1/devices/:id` | List and revoke devices (revoking your own device signs it out of sync). |
| `PUT /v1/key` `{wrapped_key, kdf_params}` | Passphrase change. |
| `DELETE /v1/users/me` | Deletes the sync account and everything in it. |

On a conflict the client decrypts the server's copy, keeps whichever has the later clock, and pushes again with the new `base_seq` if its own value won. The server never merges.

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
- **Refreshing an id_token.** The server accepts id_tokens up to 10 minutes old (`iat`) for the configured audiences. An id_token proves identity only; the data stays safe behind the passphrase even if one leaks.

## Self-hosting

`server/` builds a Node app and a Docker image (`ghcr.io/billpeet/peetmail-sync`). Configuration through environment variables:

| Variable | Meaning |
|---|---|
| `DATABASE_PATH` | SQLite file (default `/data/sync.db`). |
| `ORIGIN` | Public URL. HTTPS is required for anything not on localhost; the client refuses plain http to other hosts. |
| `ALLOW_REGISTER` | Who may create a sync account: addresses, `@domain`, or `tenant:<tid>`. Required, with no open default, since the server is reachable from the internet. Linking more accounts to an existing user is always allowed. |
| `MICROSOFT_CLIENT_IDS`, `GOOGLE_CLIENT_IDS` | Accepted id_token audiences. Default: PeetMail's built-in apps. Add your own app's ID if you use one. |
| `OIDC_ISSUERS` | Override of the issuer discovery URLs. Exists so the test harness can point the server at the fake Graph and Gmail; also lets an air-gapped install use a proxy. |

Backups are the SQLite file. A user's guide goes in `docs/user/sync.md` (setup for users) and `server/README.md` (running the server).

## Repo layout

- `pnpm-workspace.yaml` with the root app and `server/`.
- `server/`: SvelteKit (adapter-node), Drizzle, `better-sqlite3`, Tailwind for the few pages it has (a status page, later the web client). API routes under `src/routes/v1/`. Its own `pnpm check` and Vitest, wired into the root `pnpm verify`.
- Protocol types are hand-written on both sides (`server/src/lib/protocol.ts`, `src-tauri/src/sync/protocol.rs`), kept honest by shared JSON fixtures in `server/protocol-fixtures/` that a Vitest and a Rust test both read, and by the E2E tests running against the real server.
- Rust: `src-tauri/src/sync/` (`mod.rs` engine, `crypto.rs`, `protocol.rs`, `hlc.rs`).

## Stages

### Stage 1: item-shaped settings

Split `splits`, `snippets` and `signatures` into per-item keys with a migration; add the synced/local registry and its checks; `delete_setting` and `settings_with_prefix`. No visible change, so no changelog line.

**Tests.** Rust: the migration converts each old shape, skips bad entries, gives a new install and an unreadable split list the default Other tab (and keeps an empty list empty), and drops the old Important default only when it hadn't been already; order assignment (appends, inserts, moves, swaps, exhausted gaps, 200 repeated moves); saves write only changed entries; split ids survive cleaning and duplicates get fresh ones; the source scan finds an unclassified key; diagnostics count item keys without showing them. The existing E2E tests for splits, snippets and signatures pass unchanged.

### Stage 2: sync server

`server/` package, schema, the calls above, id_token verification against JWKS (cached), device tokens (random, stored hashed), `ALLOW_REGISTER`, Docker image built in CI.

**Tests.** Vitest against a temp SQLite: compare-and-swap (stale `base_seq` conflicts, new items, tombstones), paging by `seq`, per-user isolation, register allowlist, link/unlink rules (`linked_elsewhere`, last identity), revoked device rejected, expired/wrong-audience/wrong-issuer tokens rejected. A test JWKS signs the tokens.

### Stage 3: client sync and Settings → Sync

The engine, crypto, Settings → Sync (server URL, turn on, passphrase and recovery key, linked accounts, devices, passphrase change, turn off, delete sync account), footer status on failure, keymap entries for opening Sync settings and "Sync now". `docs/user/sync.md`, changelog line.

**Tests.**
- Rust: HLC ordering and clock skew, conflict resolution, encrypt/decrypt round trip, associated data stops a swapped blob, unknown keys and newer schema versions kept.
- Fake Graph and fake Gmail issue signed id_tokens and serve JWKS and discovery.
- E2E with two app instances (separate data dirs) against the real server: turn on in A, join from B with the passphrase, add a snippet in A and see it in B, edit the same split in both and the later one wins, a wrong passphrase is refused, a revoked device stops syncing.
- **Canary**, like `telemetry.spec.ts`: seed a signature, snippet and address with known strings, sync, and fail if any of them appears in the server's SQLite file or request log.

### Stage 4: synced account list

`account:<provider>:<address>` items. A device that doesn't have a synced account shows it under Settings → Accounts as "Not signed in on this device" with a sign-in button, and the footer suggests it once. Removing an account on one device asks whether to remove it everywhere.

**Tests.** E2E: an account added in A shows in B as not signed in; signing in from there works against the fake server; "remove everywhere" removes it in B.

### Stage 5: shared mailboxes and send-as

The feature from PLAN.md Tier 3, with its per-account configuration stored as synced items from the start (`shared_mailboxes:<address>`, `send_as:<address>`). Planned separately when it starts.

## Risks and open questions

- **Microsoft id_token issuer.** Multi-tenant tokens have issuer `https://login.microsoftonline.com/{tid}/v2.0`; personal accounts use tenant `9188040d-6c67-4c5b-b112-36a304b66dad`. Check with a work and an Outlook.com account before stage 2 relies on it.
- **id_token on refresh.** Entra returns one on refresh with `openid` in the scopes; confirm Google does too for the stored refresh token. If not, linking falls back to the id_token from the original sign-in, which is too old, so a Google account would need a reconnect to link.
- **Concurrent reorders.** Two devices reordering splits offline can interleave oddly. Fractional indexes keep it from losing items; the order may just need fixing by hand.
- **Error text.** Sync server errors go through `scrub` before reaching Sentry or logs, like Graph errors.
- **Web client later.** The crypto choices (AES-GCM, Argon2id via WASM) keep it possible; a web client would also need its own OAuth redirect for the id_token, so the server would accept a third audience.
