# Distribution plan

How PeetMail goes from "runs from source on one PC" to "installed and auto-updating for a small internal group", with errors, logs and traces we can actually use. Work is split into stages; each one ships on its own and leaves `pnpm verify` green.

## Status

| Stage | State |
|---|---|
| 0 Groundwork | Done |
| 1 CI + releases | Done (workflows written and linted; first real run happens when the repo is pushed) |
| 2 Auto-update, canary and stable channels | Done (same caveat) |
| 3 Sentry | Done in code and tests; uploads wait for the `SENTRY_AUTH_TOKEN` secret |
| 4 Telemetry | Done in code and tests; sends nothing until the endpoint variable and token secret are set |
| 5 Built-in sign-in apps | Microsoft done (app registered, secret set); Google waits for its client ID and secret |
| 6 Support and upkeep | Done: secrets inventory, Dependabot, changelog and What's new, user docs and Help, diagnostics and feedback, startup health checks, release checklist |
| 7 Linux | Done in code, CI and release workflow; first real release run happens with the next push |

## Decisions

| Question | Decision |
|---|---|
| Repo and releases | Public GitHub repo, MIT. Releases on the same repo, so the updater reads `latest.json` from public release assets with no token. |
| Code signing | Unsigned for now. Users click through SmartScreen once per download; updates are still verified by the updater key. Revisit Azure Trusted Signing if the group grows. |
| Telemetry backend | Self-hosted LGTM behind the OTLP gateway PolyCode already uses (see Stage 4). |
| Users | Small internal group, mostly in one Entra tenant, also using personal Outlook.com and Gmail accounts. |
| Consent | Crash reports and telemetry on by default, with a first-run notice and an opt-out in Settings. |
| Identity | Identifier `com.peetsoftware.peetmail`, publisher "Peet Software". |
| Release channels | Canary (a build of every push to `master`) and Stable (tagged releases only). |
| Linux packaging | One AppImage for every distribution, self-updating through the same channels. See Stage 7. |
| Sentry | Project `peetmail` in the `peet-software` org. DSN in the repo variable `PEETMAIL_SENTRY_DSN`; uploads use the `SENTRY_AUTH_TOKEN` secret. |
| OTLP token | Gateway `https://otlp.biap.cc` (Traefik basic auth in front of Alloy on futura). PeetMail has its own user, `peetmail`; its header is the `PEETMAIL_OTLP_HEADERS` secret and the URL the `PEETMAIL_OTLP_ENDPOINT` variable. The route only reaches Alloy's OTLP receiver, so the credential can write and nothing else. |

## Where we started

- NSIS bundle target, `com.boris.peetmail` identifier, version `0.1.0` written in three places (`package.json`, `Cargo.toml`, `tauri.conf.json`).
- No git remote, no CI.
- OAuth client IDs can already be baked in at build time (`PEETMAIL_MS_CLIENT_ID`, `PEETMAIL_GOOGLE_CLIENT_ID`, `PEETMAIL_GOOGLE_CLIENT_SECRET` via `option_env!`), with a stored setting that overrides them. There is one client per provider, global, so changing it signs every account of that provider out.
- Logging goes through the `log` crate and `tauri-plugin-log` to a file in `%LOCALAPPDATA%`. Panics are written to that file. Release builds use `panic = "abort"` and `strip = true`.
- The frontend has no error handling beyond what each call site does, and the CSP allows `connect-src` only to IPC.

## Principles

1. **One egress point.** Only the Rust core talks to Sentry, the OTLP endpoint and the update server. The webview sends errors and spans over IPC, so the CSP stays `connect-src ipc:` and there's one place to redact.
2. **Allowlist, not denylist, for telemetry.** Spans and logs carry only attributes we chose and typed. Free text (subjects, addresses, bodies, error strings from servers) never leaves the machine unless it has been through the scrubber. The scrubber exists as a backstop, not as the plan.
3. **Testable like everything else.** The harness gets a fake Sentry, a fake OTLP collector and a fake update server, the same way it has a fake Graph. A test seeds known PII into the fixtures and fails if any of it reaches a fake collector.
4. **Release builds can't be pointed elsewhere by env var.** Endpoint overrides for tests go in `testenv.rs`, as now.

---

## Stage 0: groundwork (done)

- **Identity.** Identifier `com.peetsoftware.peetmail`, publisher and authors "Peet Software", MIT `LICENSE`. Data from the old `com.boris.peetmail` folders (database and WebView2 profile) moves across on first start (`src-tauri/src/datadir.rs`), never over existing data.
- **One version source.** `package.json` holds it; `tauri.conf.json` reads `../package.json`; `pnpm version:set` copies it into `Cargo.toml` and `Cargo.lock`; `pnpm version:check` (in `pnpm verify` and CI) fails on drift, and the release checks the tag against it (`scripts/version.ts`).
- **Install mode.** NSIS `installMode: "currentUser"`: install and update need no admin. WebView2 bootstrapper left at the default.
- **Debug symbols.** The release profile adds `debug = "line-tables-only"`, so the linker writes `peetmail.pdb` (39 MB) beside a ~11 MB exe. `strip = true` stays: with MSVC it doesn't touch the PDB or the exe's CodeView record (checked: same PDB, same debug ID reference, exe 90 KB smaller). The exe embeds only the file name `peetmail.pdb`, not the build path. The PDB is attached to each release as a zip and isn't installed.
- **DB safety on upgrade.** Before migrating an existing database, `Db::open` writes `peetmail.db.v<schema>.bak` with `VACUUM INTO` and keeps the newest two. A database with a newer schema than the build knows is refused. The app shows a message box with the reason and exits, instead of crashing silently or touching the file.
- **Clippy clean.** Existing warnings are fixed and `pnpm lint:rust` (`-D warnings`) is part of `pnpm verify`.

Tests: Rust unit tests for backup, pruning, refusal and legacy-data adoption; Vitest for the version script; E2E (`tests/e2e/startup.spec.ts`) that the real exe exits with the reason logged and the newer database untouched.

## Stage 1: CI and GitHub Releases (done)

**`ci.yml`** runs on pushes to `master`, PRs, and as the first job of a release.
- `verify` (windows-latest): version check, `svelte-check`, Vitest, clippy `-D warnings`, `cargo test`, Playwright E2E headless against the fakes. One cargo target dir (`target-harness`) is shared by clippy, tests and the harness build so the Rust cache holds one set of dependencies; only `master` writes the cache. Test results are uploaded on failure.
- `audit` (ubuntu): `cargo deny` for licences, bans and sources (blocking; policy in `src-tauri/deny.toml`), advisories (reported on branches, blocking on release tags), and `pnpm audit --prod`.

**`release.yml`** runs on `v*` tags: calls CI, checks the tag against the version, builds with `pnpm tauri build` (built-in OAuth client IDs from secrets), then creates or refreshes a draft release with the installer, the symbols zip and `SHA256SUMS.txt`, and checks every asset arrived. Pre-release tags (`-beta.N`) become pre-releases. Publishing stays manual. Runbook: `docs/RELEASING.md`.

All actions are pinned by commit SHA (the release job holds a write token, and later the signing keys).

Stage 2 reworked `release.yml` into the two channels described there. Not done yet, added by later stages: Sentry release, PDB and source-map upload (Stage 3), OTLP endpoint and DSN secrets (Stages 3 and 4).

## Stage 2: automatic updates, canary and stable (done)

**Channels.** Canary is a build of every push to `master`, versioned `<next patch>-canary.<run>` (`scripts/version.ts canary`), so canaries sort after the last stable and before the next. It's published straight away as a pre-release `canary-v<version>`; then the fixed pre-release `canary` gets a new `latest.json` pointing at it, and builds beyond the newest 10 are pruned. Stable is a `v*` tag, drafted with its `latest.json`; publishing the draft makes it the repo's latest release, which is what the Stable feed (`releases/latest/download/latest.json`) reads. PolyCode's publish-on-every-push model is the canary channel; stable keeps a human in the loop. Runbook: `docs/RELEASING.md`.

**In the app** (`src-tauri/src/update.rs`, `src/lib/updates.svelte.ts`):
- `tauri-plugin-updater` with the public key in `tauri.conf.json` and `requireSignedVersion` on, so a tampered manifest can't pass an older, validly signed build off as a newer version (the signature's trusted comment carries the version; `tauri build` writes it). `tauri-plugin-process` wasn't needed.
- The feed URL is built at runtime from the channel and the repo, compiled in from CI (`PEETMAIL_UPDATE_REPO`, `PEETMAIL_CHANNEL`). Local builds have no repo, so they show "Updates aren't available in this build" and never check.
- First check a minute after start, then every 6 hours, plus Settings → Updates → Check now and a palette command. A newer build is downloaded and verified in the background; the footer then shows "0.3.0 is ready · restart to update", and `Ctrl+Shift+U` (a keymap binding, so it's in `?` and the palette) installs it.
- Install is refused while a draft is open or a send is in its undo window (`outbox::sends_in_progress`); scheduled sends persist and don't block. NSIS runs in passive mode; the app flushes its log, exits, and the new version starts.
- Settings → Updates: version, channel (Stable / Canary), status, Check now / Restart to update. A build defaults to the channel it was built for. Switching channel drops a download from the other channel and checks the new one. Canary → Stable takes effect at the next stable release (no downgrades).
- Status events carry a sequence number, so a late command result can't overwrite a newer event (a real race the E2E caught).

**Tests.** Rust: channel resolution, feed URLs, canary version ordering (with the `semver` crate the updater uses), `sends_in_progress`. Vitest: canary version rule, manifest shape, status text, out-of-order statuses. E2E (`tests/e2e/updates.spec.ts`) against a fake update server serving test-key-signed payloads: disabled without a feed; background check finds, downloads, verifies and installs via `Ctrl+Shift+U`; Check now; channel switch; tampered payload refused; mismatched signed version refused; no install with a draft open or a send pending. Install itself is faked in tests (`PEETMAIL_FAKE_UPDATE_INSTALL`); the real install-and-relaunch is on the release smoke-test checklist. A canary-versioned, signed installer was built locally to confirm NSIS accepts pre-release versions and the asset names match the workflow.

**Not done:** install-on-quit, and a `minimumVersion` field to force everyone off a bad build. Both are small additions to `update.rs` if they're needed.

## Stage 3: Sentry (done)

**What's reported** (`src-tauri/src/crash.rs`):
- Rust panics, from any thread (the SDK's panic integration; with `panic = "abort"` it captures and flushes before the process dies).
- Native crashes (access violations, stack overflows) as minidumps. Sentry's minidump integration runs a small crash-reporter process (this exe, re-run; it stops inside `sentry::init`, which is why that's the first thing `run()` does) and uploads the dump when the app dies. Minidumps hold raw stack memory, which the scrubber can't reach; Sentry doesn't keep the raw dump after processing unless the project's "Store Native Crash Reports" setting is on, so leave it off.
- Webview errors: `window.onerror` and unhandled rejections (`src/lib/errors.ts`) go to the `report_error` command, and Rust sends them as JavaScript events. The webview never talks to the network, so the CSP is unchanged. At most 20 reports a session, the same message once a minute.
- Not reported: WebView2 process crashes (they're separate processes; a `ProcessFailed` handler could add them later), and log lines on their own. Info and above are kept as a 50-line breadcrumb trail on every report. The trail is process-wide rather than in the SDK's per-thread scope, which would have dropped lines logged on sync threads.

**What's removed** (`src-tauri/src/scrub.rs`, shared with Stage 4): the signed-in accounts' addresses and names (exact, case-insensitive), any email address, bearer tokens and JWTs, OAuth parameters (`code=`, `refresh_token=`…), URL query strings (routes are kept), Windows profile paths (`C:\Users\<name>` with spaces handled), and long opaque IDs. Every event also loses its host name, device name, request and user fields except a random install ID. `send_default_pii` is off.

**Consent:** on by default; Settings → Privacy turns it off (immediately, and before the next start's crash reporter is spawned) and shows the install ID to quote. A first-run notice says reports are on, with a Settings button. Builds without a DSN (local, and tests without the fake) show neither and send nothing.

**Release:** the DSN is compiled in from the repo variable. The release job builds the frontend with hidden source maps, uploads them for `peetmail@<version>`, deletes them before `tauri build` embeds `build/`, uploads `peetmail.pdb` and `peetmail.exe` as debug files, and finalizes the release. Without the `SENTRY_AUTH_TOKEN` secret those steps warn and skip, so builds still ship.

**Tests:** Rust unit tests for the scrubber, event scrubbing, JS stack parsing, the breadcrumb trail and the install ID; Vitest for the webview hooks and rate limit. E2E against a fake Sentry (`tests/harness/fake-sentry.ts`, parses envelopes including binary attachments): a webview error arrives scrubbed with release, environment, install ID and breadcrumbs; a panic arrives with its message and trail; opting out stops reports at once and opting back in resumes them; the notice shows once; a deliberate native crash produces a real minidump (`MDMP`) from the crash reporter; and no report contains the account's address or name, a correspondent's address, the PC's name or the Windows user name.

## Stage 4: telemetry to the LGTM stack (done)

### Pipeline (`src-tauri/src/telemetry.rs`)

- OpenTelemetry SDK 0.33 with the OTLP exporter over **HTTP/JSON** to the gateway (`/v1/traces`, `/v1/metrics`, `/v1/logs`). JSON is what PolyCode's exporters already send to it, and what the harness's fake collector reads without a protobuf library. Batch processors for spans and logs (bounded queues; when the gateway is unreachable they drop rather than block), metrics every 30 s, 5 s export timeouts, shutdown on exit and before an update installs.
- Endpoint and headers are compiled in: `PEETMAIL_OTLP_ENDPOINT` (repo variable) and `PEETMAIL_OTLP_HEADERS` (secret, `key=value,key=value`). The token is extractable from the binary of a public repo, so it must be write-only, PeetMail's own, and rate-limited at the gateway.
- **Deviation from the plan:** no move from `log` to `tracing`. Spans use the OpenTelemetry API directly through `telemetry::traced(name, attrs, future)`, which carries the span's context through the future so child spans (HTTP inside a folder sync inside an account sync) nest. Logs keep going through `log`: the logger wrapper from Stage 3 also exports info-and-above lines, scrubbed, and the SDK attaches the active trace, so Loki lines link to Tempo traces. This avoided rewriting every log call and a second subscriber stack next to Sentry.
- Resource: `service.name=peetmail`, `service.namespace=peetsoftware`, `service.version`, `deployment.environment.name` (stable, canary or dev), `service.instance.id` (per process), `peetmail.install.id`, `peetmail.machine.id`, `os.type`.

#### What to reuse from PolyCode

PolyCode's `apps/desktop/src/main/observability.ts` (Node/Electron) solved several problems we'd otherwise rediscover. The Rust port should keep these:

- **`service.instance.id` is per process, not per install.** PolyCode first used the install hash. When two processes from one install overlapped (an old one lingering after an update), Prometheus saw two cumulative counters interleaved as one series, counted a reset every export, and showed rates a thousand times too high. Use `<install id prefix>-<machine id>-<pid>-<start time>`, with the install ID as its own attribute for grouping launches. The updater makes this overlap likely for us too.
- **A hashed machine ID** (SHA-256 of the lowercased hostname, first 8 hex chars) tells apart machines that would otherwise share an install ID. PolyCode derives the install ID from a hash of the user-data path. That path contains the Windows username, so its hash is guessable. PeetMail uses a random install ID stored in settings instead.
- **Byte histograms need their own buckets.** The default histogram buckets are millisecond-shaped (up to 10,000). PolyCode adds a view giving `By`-unit histograms doubling buckets from 1 KB to 4 GB. The view must also match the instrument type: matching on the unit alone turned memory gauges into histograms and hid heap size for several releases.
- **Small helper API** (`with_span`, `count`, `record_duration`, `record_size`, `record_gauge`, `record_log`) over lazily created instruments. That keeps call sites to one line, and the attribute allowlist (below) can be enforced in one place.
- **Logs carry the active span's context**, so Loki lines link to Tempo traces.
- **Only W3C `traceparent` crosses process boundaries**, never arguments. PolyCode does this for its child processes; we do it for `invoke()` from the webview.
- **Export interval 30 s for metrics, batch processors for spans and logs, flush on quit** (with a short timeout, so quitting is never held up).
- **Crash incidents share an ID** between the local log, the OTLP log and the Sentry event (`docs/crash-diagnostics.md` in PolyCode). Worth copying in Stage 3 for WebView2 process failures, which are the Tauri equivalent of Electron renderer crashes.

PolyCode's own PII handling is per-call-site discipline (hashes for IDs, allowlisted categories). PeetMail handles mail, so it adds the enforced allowlist, the scrubber and the canary test described below.

### What's instrumented

| Area | Telemetry | Attributes |
|---|---|---|
| Startup | `app.startup.duration` (process start to window), `ui.first_paint.duration` | |
| Auth | `auth.sign_in`, `auth.refresh` spans | provider, outcome, error kind, `AADSTS…`/OAuth error code only |
| Sync | `sync.run` → `sync.account` → `sync.folder` / `sync.mail` / `sync.calendar` spans; a "sync finished" log line in each run | provider, folder role, pages, items, full resync |
| HTTP | `http.request` span for every Graph, Gmail and Calendar call (one choke point, `graph::send`); `peetmail.http.throttled` counter | method, **URL template** (`/v1.0/me/mailFolders/{id}/messages/delta`), API, status, attempts |
| Outbox | `outbox.op` span (sends included) | op kind, attempt, outcome |
| Bodies | `body.fetch` span, `body.sanitize.duration`, `body.fetch.size` histogram, `peetmail.body.reads` (cache hit/miss) | size bucket |
| Search | `search.query` span | number of free-text terms and of operators, result count |
| UI | `ui.action` span per keymap binding, key press to next painted frame; `ui.long_task` (> 50 ms) | binding id, view |
| Updater, snooze | `peetmail.update.checks`, `peetmail.snooze.woke` counters | channel, outcome |
| Process (every minute) | `peetmail.outbox.depth`, `peetmail.body_cache.size`, `peetmail.db.size`, `peetmail.accounts`, `process.memory.usage` gauges | provider |

Every span also feeds a `<name>.duration` histogram (ms buckets up to 2 minutes) with its low-cardinality attributes and outcome, so Mimir has latency and error rates without querying Tempo.

### Keeping personal data out

1. **By type, not by care.** Attributes are `(Key, Val)`. `Key` is a closed enum. A `Val` is a `&'static str`, a number, a bool, or `Safe`, which only `route_template` and `code` can make. A subject, address or search term can't become an attribute without editing `telemetry.rs`, and a unit test fails if any other file builds OpenTelemetry attributes itself.
2. **URL templates, not URLs:** the path only, with every segment that isn't a fixed route word replaced by `{id}`. Hosts and query strings are dropped.
3. **Shapes, not content:** search sends counts of terms and operators; sizes are buckets; errors are a kind (`server` plus status, `timeout`, `auth` plus code), never text.
4. **Logs are scrubbed** by the same `scrub.rs` as crash reports, and only info and above leave the PC.
5. **Consent:** Settings → Privacy → "Send usage and performance data", on by default, covered by the first-run notice. Off stops recording at once; without an endpoint, or when off at start, the exporters are never created.

Planned items still open: per-account correlation (`account.ref`) isn't needed, since nothing is per account; the trace context isn't yet passed through `invoke`, so a UI action and the command it triggers are separate traces; there's no WebView2 memory gauge, `os.version` or WebView2 version, slow-query DB spans or head sampler. Grafana dashboards (`ops/grafana/`) are still to do.

### Tests

- Rust: URL templates (Graph, Gmail, Calendar, the fakes), codes, auth-error attributes, size buckets, header parsing, per-process instance IDs, search shape, and the no-raw-attributes check.
- Vitest: the webview timing batcher.
- E2E against a fake OTLP collector (`tests/harness/fake-otlp.ts`). A session (sign-in, sync, open, search, send, archive) produces the expected spans, nested correctly (`http.request` inside `sync.folder` inside `sync.account`), with templated routes, the search's shape, binding ids, the resource and install ID, the auth header on every request, metrics, and logs carrying trace IDs. **PII canaries:** none of the account address or name, correspondents' names and addresses, subjects, body text, the search word, provider message IDs, the sent message's recipient, subject and body, the ingest token, the PC name or the Windows user name appears anywhere the collector received. Opting out stops spans straight away.

## Stage 5: built-in sign-in apps, with your own as an advanced option (done)

### The built-in apps

- **Microsoft:** the existing `PeetMail` registration in the `metroid.net.au` tenant (client ID `04e0699b-c2f4-46f4-aa06-918aff940115`): any organisational directory plus personal Microsoft accounts, public client, `http://localhost` redirect. Tenant-wide admin consent covers `offline_access openid profile email User.Read Mail.ReadWrite Mail.Send Calendars.ReadWrite User.ReadBasic.All`, so nobody in the tenant sees a consent prompt; personal accounts consent for themselves. Its declared API permissions now list those nine scopes too, so admins elsewhere see what it asks for. `Place.Read.All` (room list via Places) stays optional and admin-approved, as the app already handles. Users in tenants that block user consent need their admin to approve the app, or their own app (below). Publisher verification (needs an MPN ID) can come later.
- **Google:** a Google Cloud OAuth client of type *Desktop app*, in a project with user type *External*, published *In production* and left unverified. Any Google account can sign in, behind the "Google hasn't verified this app" screen, capped at 100 users lifetime. (`gmail.modify` is a restricted scope; verification would need a CASA assessment. *Internal* would hide the warning but block personal Gmail.) Google has no CLI for desktop OAuth clients, so this one is made in the Cloud console.
- `release.yml` compiles the IDs in from the `PEETMAIL_MS_CLIENT_ID`, `PEETMAIL_GOOGLE_CLIENT_ID` and `PEETMAIL_GOOGLE_CLIENT_SECRET` secrets. Local builds have none and work as before (setup asks for a client ID); export the same variables before `pnpm tauri build` to get built-in apps locally.

### Accounts keep their app

- `accounts.oauth_app` is `builtin` or `custom` (migration 0009). Existing accounts are labelled by what they actually used: `custom` where a client ID had been saved (it took precedence over a built-in one), else `builtin`. Nobody is signed out by the upgrade.
- Token refresh and reconnect use the account's own app (`AppState::oauth_for_account`). Saving your own app signs out only the accounts that use it.
- New sign-ins use the built-in app when the build has one. Your own app is a choice: "Sign in with your own app" on the setup screen, or "Add … account with this app" under Settings → **Advanced: your own sign-in apps**. In builds without built-in apps, the client ID forms stay where they were.
- Planned but not done: a tenant lock for single-tenant custom apps (authority set to the tenant instead of `common`), several custom apps per provider, and registry policy overrides (`HKCU\Software\Policies\PeetMail`) for Intune/GPO.

Tests: Rust for the migration and for signing out only one app's accounts; E2E with a debug-only stand-in for the built-in IDs (`PEETMAIL_BUILTIN_*`): local builds still ask for a client ID; with built-in apps, Microsoft and Gmail sign in with no setup; your own app lives under Advanced and saving it signs nobody out; and with short-lived tokens, a refresh after setting a different own-app ID still uses the account's built-in app (the fake rejects any other ID, so the old global client would fail it).

## Stage 6: support and upkeep (done)

Agreed plan, in order. The first-run privacy notice is already done (Stages 3 and 4).

1. **Secrets inventory** (done: `docs/SECRETS.md`, no values): every credential the app and its releases depend on, where the live copy is, where the backup is, what breaks if it's lost or leaks, and how to rotate it. The urgent part is the updater signing key: it has to be in a password manager as well as GitHub, because losing it means no installed copy can update again.
2. **Dependabot** (done): weekly, minor and patch grouped per ecosystem (cargo, npm, GitHub Actions), majors as their own PRs; vulnerability alerts and security updates on.
3. **Changelog and "What's new"** (done; `src/lib/changelog.ts`, `WhatsNew.svelte`). `CHANGELOG.md` with `## Unreleased` and one section per version, written for users. AGENTS.md: every user-visible change adds a line in the same commit. `pnpm version:set` turns Unreleased into the version's section; a stable release fails without one and uses it as the release notes and the update's notes. The changelog is compiled into the app: Help shows it, and after an update a "What's new" panel shows the lines not seen yet (tracked per line, so it works for canary, stable and skipped versions alike).
4. **User docs** (done; Help is `Help.svelte`, opened with F1; Settings was restructured into pages alongside it) in `docs/user/` (getting started incl. SmartScreen, updates and channels, privacy, your own sign-in app, troubleshooting), rendered by GitHub and compiled into the app's Help view, so they match the installed version and work offline. Markdown is rendered through a sanitiser. AGENTS.md: docs change in the same commit as the behaviour. Keyboard shortcuts stay in the `?` overlay (generated from the keymap).
5. **Diagnostics export** (done; `src-tauri/src/support.rs`): Help → Report a problem → Export writes `PeetMail-diagnostics-<version>-<time>.zip` to Downloads: `summary.json` (app version, channel, install ID, Windows version from the registry, WebView2 version, update and privacy status, DB schema version, size, `quick_check`, row counts per table, accounts as provider/app/signed-in only, outbox counts by op kind and state, settings with values only for plain switches and numbers, `[set]` for the rest) and the newest log files scrubbed by `scrub.rs`, 2 MB at most. Rust tests check the summary has no addresses, names, split queries or secrets; E2E unzips a real export and checks the mailbox's addresses, names and subjects aren't in it.
6. **Feedback** (done): Help → Report a problem (also in the palette). The form sends a Sentry `feedback` envelope item (the Rust SDK has no type for it, so `support::feedback_envelope` writes it) through a client made just for it, so it works with crash reports off and the user's words aren't scrubbed (the form says so). It carries the install ID, release and channel, an optional reply address, and the diagnostics zip as an attachment unless unticked. It isn't tied to breadcrumbs; the install ID finds the same install's crash reports. "Prefer email?" opens a draft in PeetMail (or the default mail app with no account) to `PEETMAIL_FEEDBACK_EMAIL` with the version and install ID; builds without the variable hide it, and builds with neither say to export diagnostics and send them to whoever gave you PeetMail. E2E covers all three against the fake Sentry.
7. **Startup health checks** (done; `src-tauri/src/health.rs`, `HealthBanner.svelte`): at startup and every 6 hours, in the background. The database's `quick_check` after a migration (its own read-only connection, so a big file doesn't stall the app), free space under 500 MB, a write probe in the data folder, the clock against Microsoft Graph's `Date` header (over 5 minutes off; offline means no verdict), and WebView2 older than 120. Each problem is a banner (red for errors, amber for warnings) saying what to do, with Help → Troubleshooting covering each one; dismissing hides it for the session ("Dismiss warnings" in the palette hides them all). Each finding counts `peetmail.health.issues` with `peetmail.health.check`. Measuring and deciding are separate: Rust tests cover the decisions and the real measurements; E2E runs the real checks (the clock against the fake Graph) and, through the debug-only `PEETMAIL_FAKE_HEALTH`, every banner and the telemetry count.
8. **Release checklist** (done): "Is the canary ready?" in `docs/RELEASING.md`: two days on canary (a week for migrations, sign-in or sync changes), no new Sentry issues or open feedback from canary, telemetry against the last stable week, a canary self-update, the migration path from the last stable in Sandbox, and the changelog and Help read through. The smoke test also covers health banners, Help, What's new and Report a problem.

## Stage 7: Linux (done)

### Packaging: one AppImage

Linux users get `PeetMail_<version>_amd64.AppImage`. It was picked over the alternatives because it's the only format that is one file for every mainstream distribution *and* that Tauri's updater can replace in place without root, which keeps updates working exactly like Windows (background download, signature check, "restart to update", the same Stable and Canary feeds).

| Option | Why not (for now) |
|---|---|
| `.deb` / `.rpm` | One per distro family, and the updater installs them through `pkexec` (a root password prompt on every update). Arch needs a third (AUR). Worth adding later for people who want the package manager to own it, with updates left to a repo. |
| Flatpak (Flathub) | Cross-distro, but updates come from Flathub on its schedule and our channels don't map onto it; the sandbox also needs portals for the keyring, tray and opening attachments. A good second channel later. |
| Snap | Ubuntu-centric; same update-ownership problem. |

The AppImage is built on Ubuntu 22.04 (the oldest supported LTS): it bundles WebKitGTK and GTK and links against 22.04's glibc, so it runs on anything from 2022 on (Debian 12, Fedora, Arch, Mint, openSUSE Tumbleweed...). It needs FUSE 2 on the host, which Ubuntu 24.04+ no longer installs by default; the help says how, and `APPIMAGE_EXTRACT_AND_RUN=1` works without it. x86_64 only for now; an aarch64 build is an `ubuntu-22.04-arm` matrix entry away.

### What changed in the app

- **Saved sign-ins** (`secure.rs`): DPAPI has no Linux equivalent, so tokens are sealed with ChaCha20-Poly1305 (ring, already in the tree) under a random 256-bit key kept in the Secret Service keyring (GNOME Keyring, KWallet 5.97+, KeePassXC) via the `secret-service` crate (zbus 5, also already in the tree). A locked keyring is unlocked through its own prompt; a dismissed prompt is an error that background work doesn't repeat for five minutes (signing in asks again at once), never a reason to fall back. Only a desktop with no keyring service at all (a bare window manager, a server) gets the key in `token.key` with mode 0600, like Chromium's basic store. A marker file records that the key is in the keyring, so a keyring that later won't answer is an error rather than a reason to mint a second key that orphans the saved tokens. A damaged key file is set aside so the user can sign in again. Test runs (`PEETMAIL_DATA_DIR`) never touch the real keyring.
- **Private files**: the data folder is made 0700 (some distros' home folders are world-readable), and attachments opened with a double-click are copied to PeetMail's own cache folder (0700) rather than the shared `/tmp`. Linux launchers, scripts and packages (`.desktop`, `.sh`, `.AppImage`, `.deb`...) join the list of attachments PeetMail won't open directly.
- **Updates** (`update.rs`): only an AppImage in a folder the user can write to updates itself (`APPIMAGE` is set by its runtime); anything else says why and never checks. After installing, the app restarts itself (NSIS does that on Windows); `AppHandle::restart` relaunches the AppImage file, not the mounted old copy, and a flag in the database makes the new version show its window even if the old one was started hidden by autostart. The manifest lists the AppImage under `linux-x86_64-appimage` and `linux-x86_64`.
- **Platform code** behind `cfg`: the fatal-start message box (zenity or kdialog, falling back to stderr: GTK bindings directly would be an unmaintained direct dependency), free disk space (`statvfs`), memory gauge (`/proc/self/statm`), OS name in diagnostics (`/etc/os-release` and the kernel, plus desktop, session type and whether it's an AppImage), `os.type` in telemetry, and the early data dir for the crash reporter (`$XDG_DATA_HOME`). The WebView2 version check is Windows-only (WebKitGTK's version numbers aren't comparable). The scrubber removes `/home/<user>` and `/var/home/<user>` paths as it does Windows profile paths.
- **UI**: "Start with Windows" reads "Start when you log in" on Linux. Everything else (custom title bar, tray, notifications, autostart, single instance) works through Tauri's Linux backends; tray icons need an AppIndicator-capable desktop, so the help covers plain GNOME.
- **Debug symbols**: the Linux release build keeps line tables, and `scripts/split-debuginfo.sh` (Tauri's `beforeBundleCommand`) moves them into `peetmail.debug` before bundling. That file is uploaded to Sentry and attached to the release as a zip, the same as the Windows PDB.

### CI and release

- `ci.yml` has a `linux` job on Ubuntu 22.04: clippy `-D warnings`, `cargo test`, a debug AppImage build and `tests/linux/smoke.sh`. The Playwright suite stays Windows-only: it drives WebView2 over CDP, which WebKitGTK doesn't speak. `cargo deny` checks both targets.
- `release.yml` is now `version` → `build` (matrix: windows, linux) → `publish`. Each build uploads its signed artifact, `.sig` and symbols; `publish` writes one `latest.json` with both platforms (`scripts/update-manifest.ts`), checksums, finalizes the Sentry release and creates the canary or draft stable release as before. The release AppImage is smoke-tested before upload.

### Checked by hand while building it

In an Ubuntu 22.04 container with Xvfb: the debug AppImage signs in against the fake Graph, syncs, renders mail and calendar, and takes keyboard shortcuts (`?`, `Ctrl+K`, `Ctrl+Shift+U`). A real self-update: a 0.1.0 AppImage found 0.1.1 on a local feed, verified it, installed it over its own file on `Ctrl+Shift+U`, and came back as 0.1.1. With GNOME Keyring unlocked, the key landed in the keyring, and a restart decrypted the saved sign-in and synced. The release AppImage (83 MB; a 21 MB stripped binary, 83 MB of split-out debug info with a matching build ID) starts on Debian 12, Ubuntu 24.04, Fedora 44 and Arch (glibc 2.36 to 2.44) as well as the Ubuntu 22.04 it was built on. Like any AppImage it takes fonts, X11/Wayland client libraries and Mesa (including `libGLESv2`, which WebKitGTK loads at run time) from the host: every desktop install has them, minimal containers don't.

### Not done

- No E2E suite on Linux. `tauri-driver` with WebKitWebDriver could run a subset; the harness would need a WebDriver backend next to its CDP one.
- `.deb`/`.rpm`/Flatpak, aarch64, and an app-menu entry without a helper tool (AppImages don't install a `.desktop` file themselves).
- The Linux jobs run on GitHub's `ubuntu-22.04` image, which goes when 22.04 leaves standard support (April 2027). Moving to an `ubuntu:22.04` container on `ubuntu-latest` keeps the same glibc floor.
- Clicking the tray icon on Linux opens its menu rather than the window (AppIndicator has no click event); the menu's **Open PeetMail** does it.

## Suggested order

| Stage | Depends on | Size |
|---|---|---|
| 0 Groundwork | – | S |
| 1 CI + releases | 0 | M |
| 2 Auto-update | 1 | M |
| 3 Sentry | 1 (for symbols, source maps) | M |
| 4 Telemetry | 3 (shares scrubber, install ID) | L |
| 5 OAuth apps | 1 (secrets) | M |
| 6 Rest | 1–5 | M, can be picked off individually |

Stage 5 is independent of 2–4 and can run in parallel or go first if the setup screen is the bigger barrier for users. The first build handed to anyone should include at least 1, 2, 3 and 5; telemetry can follow in an update, which is one more reason to have the updater early.

## Open questions

Answered ones are under Decisions at the top. Nothing is open right now; Sentry and the OTLP token get set up when their stages start.
