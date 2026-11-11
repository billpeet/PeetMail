# Testing PeetMail

Four layers, fastest first. `pnpm verify` runs them all.

| Layer | Command | Covers |
|---|---|---|
| Types | `pnpm check` | Svelte, TS, and the harness (`tests/**`) |
| Unit (TS) | `pnpm test:unit` | Pure modules in `src/lib`, files named `*.test.ts` |
| Unit (Rust) | `pnpm test:rust` | `#[cfg(test)]` modules: SQL queries, PKCE, sanitizer, Graph parsing |
| E2E | `pnpm test:e2e` | The real app end to end against the fake Graph server |

## Linux

The E2E suite needs WebView2's DevTools protocol, which WebKitGTK doesn't have, so it runs on Windows only. On Linux, CI's `linux` job (`.github/workflows/ci.yml`) runs clippy and the Rust tests, builds a debug AppImage, and starts it with `tests/linux/smoke.sh` (Xvfb, a throwaway home folder): the window opens, the database is created, sync and the health checks run. The release build runs the same smoke test on the AppImage it ships.

To check Linux code from Windows, use WSL or a container on Ubuntu 22.04 with the packages from the README; copy the repo into the Linux filesystem (not `/mnt/c`) and run `pnpm install`, `pnpm lint:rust`, `pnpm test:rust` and `pnpm tauri build --debug --bundles appimage` there. The debug AppImage honours the same `PEETMAIL_*` test knobs, so it can be pointed at the fakes (`node tests/harness/fake-graph.ts`) and driven with `xdotool` on an Xvfb display. With no `PEETMAIL_DATA_DIR`, saved sign-ins use the desktop keyring (`gnome-keyring-daemon --unlock` in a `dbus-launch` session gives one).

## How E2E works

```
Playwright ──CDP──▶ WebView2 in peetmail.exe (debug build, headless)
                         │ Rust core
                         ▼
              fake Microsoft (tests/harness/fake-graph.ts)
              OAuth authorize/token + Graph /me, inbox, delta, bodies
```

- **Fake Microsoft.** `fake-graph.ts` implements the identity platform and the Graph routes PeetMail calls, with real semantics: PKCE verification, token expiry and rotation, delta paging with next/delta links, partial and `@removed` items, 429 throttling, 410 resync. Only `CLIENT_ID` (`11111111-2222-3333-4444-555555555555`) is accepted. Any other ID never redirects back, like Entra.
- **Fixtures.** `fixtures.ts` builds a seeded mailbox with dates relative to now. Messages are spread over Inbox and Sent Items, and notification senders are classified Other. Messages tests can target by id: `msg-hostile` (scripts and handlers to strip), `msg-newsletter` (remote images, Other), `msg-plain` (text body), `conv-thread-1` (4 received + your Sent reply), `msg-attach` (inline `cid:` image + PDF attachment), `msg-invite` (meeting invite), `msg-sent-*` (mail you sent, for contact suggestions).
- **Isolation.** `app.ts` launches the exe with `PEETMAIL_DATA_DIR`, its own `WEBVIEW2_USER_DATA_FOLDER`, the fake server's URLs, and a free DevTools port. Every test gets a fresh app and a fresh fake server, so tests run in parallel.
- **Headless.** The window is created hidden, so nothing appears on screen or takes focus. WebView2 still renders, and screenshots and keyboard input over CDP work. Set `PEETMAIL_HEADED=1` to watch.
- **Fake updates.** `fake-update.ts` serves update manifests per channel (`/stable/latest.json`, `/canary/latest.json`) built by `scripts/update-manifest.ts`, plus a payload signed with the test key in `tests/harness/updater/` (signed for version `9.9.0`). The harness build trusts that key and allows plain http through `src-tauri/tauri.harness.conf.json`, so the real download-and-verify code runs. Installing writes `{version, bytes}` to a file instead of running an installer (`PEETMAIL_FAKE_UPDATE_INSTALL`).
- **Fake Sentry.** `fake-sentry.ts` accepts crash-report envelopes (`PEETMAIL_SENTRY_DSN`) and keeps them raw and parsed, including binary attachments such as minidumps. `test.use({ withSentry: true })` gives `sentry`: `sentry.events()`, `sentry.attachments()`, and `sentry.allText()` for "this must not appear anywhere" checks. Without it the app has no DSN and sends nothing.
- **Fake OTLP collector.** `fake-otlp.ts` accepts usage telemetry (OTLP/HTTP JSON, `PEETMAIL_OTLP_ENDPOINT`) and flattens it: `telemetry.spans()` (with parent ids and attributes), `telemetry.metrics()`, `telemetry.logs()`, `telemetry.allText()` for leak checks, and `telemetry.requests` (with headers). `test.use({ withTelemetry: true })` gives `telemetry`, with exports every 200 ms; invoke `debug_flush_telemetry` to export immediately.
- **Builds.** `global-setup.ts` rebuilds the debug exe (with `--config src-tauri/tauri.harness.conf.json`) when sources are newer than the last harness build. `PEETMAIL_SKIP_BUILD=1` skips that check.

## Writing an E2E test

Import from `tests/e2e/fixtures.ts`:

- `page` is the app, not signed in. `inbox` is signed in with the first sync finished.
- `fake` is the in-process fake server. Use `fake.control(route, body)` to change the world and `fake.requestsSince(n)` to assert on traffic.
- Helpers: `syncNow`, `waitForSyncIdle`, `row(page, conversationId)`, `selectedRow`, `messageBody(page, n)` (frame locator for a message iframe), `snap(page, testInfo, name)` for a named screenshot, `inboxConversations(pred)` for expected counts, `selectWithKeys(page, conversationId)` to select a row without opening it (a click opens the conversation, which also marks it read).
- The default list is the built-in Inbox tab (mail the default `Other` split doesn't claim); `DEFAULT_CONVERSATIONS` is its expected size.
- Writes the app makes (sends, replies, moves, patches) land in `fake.state.sent`; `fake.control("message?id=...")` returns the server's copy of a message. Triage ops leave after a 2s grace period, so poll with `expect.poll(..., { timeout: 10_000 })`.
- Attachments download to `<dataDir>/downloads` in tests (`PEETMAIL_DOWNLOAD_DIR`).
- Built-in sign-in apps: tests run like a local build (no apps built in, so setup asks for a client ID). `test.use({ builtinApps: true })` makes the fakes' client IDs the built-in ones (`PEETMAIL_BUILTIN_*`), like a release build.
- Updates: `test.use({ withUpdates: true })` gives `updates` (the fake update server). `updates.publish("stable", { version: SIGNED_VERSION })` puts a release on a feed; `updateFeed: { stable: {...} }` does it before the app starts, and `updateFirstCheckSecs` sets when the app first checks by itself (default an hour, so tests use Settings → Check now). `updates.installedPath` is where an install lands.

```ts
test("new mail appears after Ctrl+R", async ({ inbox: page, fake }) => {
  await fake.control("deliver", { subject: "Hot off the press" });
  await syncNow(page, fake);
  await expect(page.getByTestId("thread-row").first()).toContainText("Hot off the press");
});
```

Drive the app the way a user does: keyboard first (`page.keyboard.press("j")`), selectors by `data-testid`. Wait on observable state (`toHaveAttribute("data-state", "idle")`, a visible element) before the next step, since saves and syncs are async.

The launcher waits until WebView2 actually delivers keyboard input before handing the page over: on the first launches of a freshly built exe it silently drops keys sent over DevTools for a few hundred milliseconds.

Every test attaches `final-screen.png`, `app.log` and `console.log` to the report. On failure, read `test-results/e2e/<test>/error-context.md` for the error and an accessibility snapshot, or open `test-results/e2e-report/index.html`.

## Benchmarks

`pnpm bench:open [runs] [warm|cold] [latencyMs]` times Enter-to-visible for large emails (an 840 KB newsletter with 900 remote images, 4 MB of embedded images) in the real app. `warm` uses prefetched bodies; `cold` blocks prefetch so bodies are fetched and sanitised on open, with simulated Graph latency. It exits non-zero when over budget. Run it after touching body fetching, sanitising, prefetch or `MessageBody`.

## The drive CLI

`pnpm drive` keeps one app + fake server running between shell calls, for exploring, reproducing bugs and checking UI changes. `pnpm drive help` lists every command. Typical loop:

```sh
pnpm drive start                 # headless, signed in, synced. --fresh stops at setup, --dev hot-reloads
pnpm drive key j j Enter         # prints a JSON state line after each action
pnpm drive body                  # text of the open message
pnpm drive shot                  # prints a PNG path; read it to see the screen
pnpm drive graph deliver '{"subject":"Hi"}'
pnpm drive key ctrl+r
pnpm drive logs
pnpm drive stop
```

`pnpm drive restart` relaunches the app on the same data, which tests persistence. `pnpm fake-graph` runs the fake server on its own, verbose, on port 4010.

## Fake server control API

`POST /_test/<route>` with JSON. `GET /_test` lists them all. The main ones: `deliver` (with optional `to`, `cc`, `folder`, `inference`), `update` (partial change such as `isRead`), `move`, `remove`, `throttle`, `config` (`rejectDeltaFilter`, `expireDelta`, `tokenTtl`, `latencyMs`, `failBody`, `failInvite`, `failWrites`), `expire-tokens`, `reset`, `requests`, `sent`, `message?id=`. Anything under `/img/` on the fake returns a PNG, standing in for a sender's image host.

When the app starts calling a new Graph endpoint, add it to `fake-graph.ts` with the real API's semantics, including its error cases, then test it through the app.
