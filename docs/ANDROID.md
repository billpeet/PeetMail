# Android plan

PeetMail on Android phones, built from the same Tauri 2 app: the Rust core unchanged where it can be, platform code behind `cfg`, and the Svelte UI gaining a compact touch layout next to the desktop one. Work is split into stages; each one ships on its own and leaves `pnpm verify` green. Stages 0–6 together are v1: nothing goes to the phone as a release until background sync works.

## Scope of v1

| | |
|---|---|
| Devices | Phones. Tablets and Chromebooks aren't targeted; they get whichever form factor their width picks, untested. |
| Android version | Android 17 and newer (`minSdk` = `targetSdk` = Android 17's API level). |
| Accounts | Microsoft 365 and Outlook.com. Gmail too if the Stage 0 sign-in spike works; it doesn't block the release. |
| Background sync | Yes. New mail arrives and notifies with PeetMail closed. |
| Distribution | A signed APK from GitHub Releases, installed by hand. No Play Store. |

## Status

| Stage | State |
|---|---|
| 0 Spike: it builds and signs in | Done (emulator; Gmail and a real phone still to try) |
| 1 Rust core on Android | Done |
| 2 Compact layout (testable on Windows) | Done |
| 3 Touch: gestures, sheets, back button | Done |
| 4 Phone-native plumbing (files, share, notifications, secure storage) | Done (the Microsoft deep link waits on its redirect URI being registered) |
| 5 Background sync | Not started |
| 6 Distribution | Not started |

## Decisions

| Question | Decision | Why |
|---|---|---|
| Separate app or same codebase | Same repo, same crate, same SvelteKit app. `tauri android init` adds `src-tauri/gen/android`. | The Rust core (sync, outbox, SQLite, sanitising, settings sync) is most of the value and has no UI in it. `lib.rs` already has `#[cfg_attr(mobile, tauri::mobile_entry_point)]` and the crate already builds `cdylib`. |
| Where the platform split lives in Rust | `#[cfg(desktop)]` / `#[cfg(mobile)]` for Tauri features (tray, windows, updater), `#[cfg(target_os = "android")]` for OS code. Android is a third arm, never a fallback. | Android is `unix` but **not** `target_os = "linux"`, so today it silently falls into `secure.rs`'s "not implemented" arm and fails to compile wherever only two arms exist (`health::free_space`, `telemetry.rs`, `support.rs`). |
| One UI or two | One UI with a **form factor**: `desktop` (today's) or `compact` (single pane, touch). Chosen by width, not by OS, with a test knob to force it. | A phone is always compact, and so is a narrow desktop window. Most mobile UI can then be built and tested on Windows with the existing harness. |
| Keyboard first on a phone | Touch first, but every touch control dispatches a keymap binding by id (`keymap.run("archive")`), never its own code path. | Keeps "every action is a binding" true: the palette, the `?` overlay, hardware keyboards (DeX, Chromebook, Bluetooth) and the E2E tests all keep working. |
| Sign-in | Stage 0 keeps the loopback redirect as is. Microsoft moves to a custom-scheme redirect (deep link) in Stage 4. Google is a spike (see Risks). | RFC 8252 loopback works on Android and needs no code change, so the spike proves the rest of the stack first. The browser tab left behind and the manual switch back are what a deep link fixes. |
| Token storage | Android Keystore holds a non-exportable AES key that wraps the data key; the existing `sealed` (AES-GCM, `ring`) code encrypts the blob. Exposed to Rust through a small Kotlin plugin. | Same shape as the Linux arm (keyring key + `sealed`), so `protect`/`unprotect` keep their signature. |
| Background sync | In v1: a WorkManager periodic job (15 min is Android's floor), plus sync while open and on resume. No push. | Android stops a backgrounded process's timers. Graph push needs a public webhook holding mail tokens, and the sync server is built never to see tokens (`docs/SYNC.md`). |
| Updates | No `tauri-plugin-updater` on Android (it's desktop only). PeetMail checks the same release feed, downloads the new APK, verifies it and hands it to the system installer, which asks you to confirm. See Stage 6. | A sideloaded app can't update itself silently. |
| Crash reports | Rust panics through the existing `sentry` setup; minidumps off on Android (they need a second crash-reporter process). | Native crashes and ANRs would need `sentry-android`; revisit after real use. |
| Minimum Android version | Android 17. | One target version: current Android System WebView, the notification permission, edge-to-edge and predictive back all behave one way, with no compatibility branches. It's your phone; widening later only means testing older versions. |
| Defaults that change on a phone | Body cache 100 MB (desktop 500 MB), mail window unchanged at 90 days. | Storage, not metadata, is the constraint; metadata for 90 days is a few MB. |
| Settings sync | Works unchanged and is the recommended way to set up the phone: join with the passphrase, get signatures, snippets, splits and the account list, then sign in to each account. | Tokens never sync, by design. |

## Stage 0: spike, it builds and signs in

### What we found (2026-11-11, Android 17 emulator)

- **It builds and runs.** `rusqlite`, `ring`, `argon2`, `ammonia` and `reqwest` + rustls compile with NDK 29 and work. The app reaches the setup screen, signs in, and syncs the inbox. IPC and the sandboxed body iframe work with the existing CSP; the page is served from `http://tauri.localhost`.
- **NDK 29 has no compiler for API 37.** The Rust library is built for Tauri's default API level; gradle's `minSdk = 37` is what keeps PeetMail off older phones. Native code built for a lower level runs unchanged on 17.
- **Android 17 cuts backgrounded apps off the network** (`dumpsys netpolicy`: `blocked=APP_BACKGROUND`, allowed only while in front). This shapes three things:
  - Sign-in: the loopback redirect arrives while Chrome is in front, so the token request fails. On mobile, `auth::sign_in` now keeps the code and retries every 2 s for up to 5 minutes, and the browser page says "Go back to PeetMail to finish". The deep-link redirect (Stage 4) is the real fix.
  - The first sync just after sign-in failed once with the same error, then worked on the next cycle.
  - Background sync can only work through WorkManager (Stage 5), which the system lets onto the network; a plain background timer gets nothing.
- **Saved sign-ins** use the Linux "no keyring" path for now: a random key in a private file in the app's folder. Android Auto Backup and device-to-device transfer are off (`data_extraction_rules.xml`), so the key never travels with the tokens. Stage 4 moved the key into the Keystore.
- **Crash reporting** starts in `setup` on Android (the data folder is only known there); minidumps are off.
- Notifications need `notification:default` in the capability (the plugin's JS checks permission at startup).

### Building it

Needs Android Studio's SDK with platform 37, build-tools 37 and NDK 29 (`ANDROID_HOME`, `NDK_HOME`), plus `rustup target add aarch64-linux-android x86_64-linux-android`.

```sh
# x86_64 for the emulator, aarch64 for a phone. The built-in sign-in apps come from the same
# env vars CI uses (PEETMAIL_MS_CLIENT_ID, ...); without them the setup screen asks for an ID.
pnpm tauri android build --debug --apk --target x86_64
adb -s emulator-5554 install -r src-tauri/gen/android/app/build/outputs/apk/universal/debug/app-universal-debug.apk
# Clippy for the Android arms (CC/AR/linker pointed at the NDK's clang, as tauri does):
cargo clippy --manifest-path src-tauri/Cargo.toml --target aarch64-linux-android --all-targets -- -D warnings
# The app's own log:
adb shell run-as com.peetsoftware.peetmail cat logs/PeetMail.log
```

### The plan as written

Goal: an APK on a real phone that signs in to a Microsoft account and shows the inbox in today's (unusable, desktop-sized) UI. Answers the risky questions before any UI work.

- Toolchain on Windows: Android Studio, SDK + NDK, `JAVA_HOME`, `rustup target add aarch64-linux-android x86_64-linux-android`. Note what Tauri needs on Windows (symlinks / Developer Mode) in `tests/README.md`.
- `pnpm tauri android init`; commit `src-tauri/gen/android` (minus build output). Add `tauri.android.conf.json` for Android-only config, merged like `tauri.linux.conf.json`.
- Get it compiling with the minimum `cfg`: tray, `WebviewWindowBuilder` sizing and decorations, single-instance, autostart and updater behind `#[cfg(desktop)]`; `tray-icon` moved to a desktop-only `tauri` feature in `Cargo.toml`.
- `pnpm tauri android dev` against a device: `TAURI_DEV_HOST` is already wired in `vite.config.js`.
- Check by hand, write down results here:
  - `rusqlite` (bundled), `ring`, `argon2`, `ammonia` and `reqwest` + rustls build with the NDK and work.
  - The CSP origins on Android (`ipc:`, `http://ipc.localhost`, and whatever the Android WebView serves the app from) still let IPC through; the sandboxed `srcdoc` iframe renders bodies and still blocks remote images.
  - Loopback sign-in: the opener plugin opens Chrome, the redirect reaches the listener while PeetMail is in the background.
  - Cold start time and memory on a mid-range phone.

## Stage 1: Rust core on Android

Done. Every module builds for Android with its own arm (never a silent fallback), and `pnpm lint:android` (clippy plus a test build for `aarch64-linux-android`) runs on any PC with the NDK and in CI's `android` job.

| Module | Android arm |
|---|---|
| `lib.rs` | No tray, close-to-tray, `--minimized`, single-instance, autostart or updater plugin. One full-screen window. A fatal startup error is logged and the app closes; Android refuses to install an older APK over a newer one, so "database from a newer version" can't happen there. |
| `lifecycle.rs` | New. Tracks whether PeetMail is in front (`WindowEvent::Suspended`/`Resumed`): going to the background sends the outbox while the network is still there; coming back syncs at once; the 60 s sync timer skips its runs while in the background (Android has PeetMail offline then). Notifications show only while PeetMail isn't in front. |
| `datadir.rs` | `app_data_dir()` is `None` on Android; crash reporting starts in `setup` once Tauri knows the folder. No dev-copy of the data. |
| `secure.rs` | The Linux "no keyring" path: a private key file. Keystore wrapping is Stage 4. |
| `health.rs`, `telemetry.rs` | Share the Linux arms (`statvfs`, `/proc/self/statm`). |
| `support.rs` | "Android 17 (API 37)" in diagnostics, from the system properties. |
| `crash.rs` | No minidumps. |
| `update.rs` | No Tauri updater; updates show as off, with "install the new APK" as the reason. |
| `commands.rs` | Autostart reads as off and can't be set. Saving and opening attachments and saving diagnostics say they aren't available on Android yet, rather than writing to a folder no one can browse (Stage 4: "Save to" picker, `FileProvider`). |
| `state.rs` | Body cache defaults to 100 MB (desktop 500 MB). |
| UI | `platformOf` tells Android from Linux. Settings hides the tray and autostart rows; `Ctrl+Q` (quit) is desktop only. |

## Stage 2: compact layout, testable on Windows

Done. One set of components with two layouts; `layout.compact` (`src/lib/layout.svelte.ts`) is true on Android and on any window 640 px wide or less, so the E2E suite drives the phone layout on Windows (`test.use({ phone: true })`, `tests/e2e/compact.spec.ts`) and `pnpm drive start --phone` shows it. Checked on the Android 17 emulator against a real mailbox too.

- **Shell** (`CompactShell.svelte`, rendered instead of the rail, panes and footer): a header (mailboxes drawer, title, sync indicator, search), the split tabs as a scrolling strip, the list or the calendar's day, a floating compose button, and a bottom bar (Mail, Calendar, Search, Settings). No title bar, reading pane, right pane or resize handles. The mailbox list (`src/lib/mailboxes.ts`) is shared with the desktop rail.
- **List**: three-line rows (sender and time, subject, preview), 84 px tall; no hover actions (swipes come in Stage 3).
- **Reader**: full screen; back and previous/next on top; archive, delete, snooze, reply and a "more" menu (reply all, forward, flag, unread, spam, load images) at the bottom.
- **Mail at phone width**: the body's own CSS already squeezes tables and images; whatever still can't shrink (fixed `min-width` blocks, unbreakable rows) is zoomed out to fit (`fitScale`, floor 0.4), refitted when the frame's width changes.
- **Dialogs**: every backdrop and box carries `dialog-backdrop` and `dialog-panel` (a full-screen page) or `dialog-sheet` (full width, from the top, clear of the keyboard); `app.css` does the rest. Settings and Help are a list of sections, each opening on its own page. Key hints on buttons and hint lines (`key-hints`) are hidden.
- **Composer**: full screen; formatting tools on one row, the send buttons on the next.
- **Safe areas**: `viewport-fit=cover`, and `--safe-top`/`--safe-bottom` from `env(safe-area-inset-*)` pad the header, bottom bar and dialogs. Works edge to edge on Android 17's WebView.
- **Back button** (pulled forward from Stage 3): Android's back sends Esc, so it does what Esc does (close a menu, the drawer, a dialog, a Settings section, leave the conversation or the search); with nothing to close it returns to the inbox, and from the inbox PeetMail goes to the background (`move_to_background`, `moveTaskToBack`) instead of closing. The undo toast has an Undo button.
- **Not done here**: the week view, agenda and 3-day calendar views (the day view is the phone's calendar for now); collapsing To/Cc into one row; a pinch-zoom for mail bodies. Revisit after using it.

## Stage 3: touch

Done. What a finger's movement means lives in `src/lib/gestures.ts` (unit-tested); the components only feed it positions.

- **List rows** (`ThreadList`, touch pointers only, `touch-action: pan-y` so scrolling stays native): drag right past the threshold (a third of the row, at most 120 px) to archive, left to snooze; what letting go will do shows under the row, which slides back if you stop short. A drag that leans vertical scrolls instead. Drafts don't swipe.
- **Long press** (450 ms) starts a selection, or adds to it; while selecting, taps add or remove rows and the header turns into the selection bar (archive, delete, read, snooze). A long press doesn't open the browser's context menu.
- **Pull to refresh**: pulling down at the top of the list (damped) past 64 px syncs, as Ctrl+R does.
- **Flick between conversations** in the reader: left for the next, right for the previous (`j`/`k`), over the mail too. Message bodies are iframes that keep their touches, so `MessageBody` classifies its own flicks and hands them up as a `pm-swipe` event.
- Back and the undo button: done in Stage 2.
- Tests: `tests/e2e/compact.spec.ts` drives touch through DevTools (`Input.dispatchTouchEvent`): swipes both ways, a short swipe, a vertical drag, long press and taps, pull to refresh, flicks over the subject and over the body. Checked by hand on the emulator with real touch input too, which found a bug the DevTools touches didn't: Android sends no click after a long press or a drag, so "ignore the click after a gesture" has to expire.
- Not done: Android 17's predictive back animation (the previous screen peeking during the gesture) is untried with the WebView; configurable swipe actions.

## Stage 4: phone-native plumbing

One Kotlin class does what Rust can't alone: `gen/android/app/src/main/java/com/peetsoftware/peetmail/PeetPlugin.kt`, registered as the `peetmail` Tauri plugin by `src-tauri/src/android.rs`. It lives in the app module, not a plugin crate, since nothing else uses it. Rust calls it with `run_mobile_plugin`, which blocks until Kotlin answers on Android's main thread, so it's never called *from* the main thread (sync commands run there).

What other apps hand PeetMail (shares, the sign-in redirect, notification taps) arrives as intents. PeetPlugin keeps them in `pending` and fires `peetmail-pending` on the page. The page calls `take_pending` once startup has loaded the accounts, and again on each event (`src/lib/android.svelte.ts`).

- **Keystore**: the 32-byte key for saved sign-ins is random, stored as `token.key.wrapped`, and encrypted by an AES-GCM key that never leaves the Android Keystore (`secure.rs`, `wrapped`). The plain `token.key` from Stages 0–3 is moved into it on first run (checked on the emulator). A lost Keystore key or a damaged file means a new key and signing in again; any other Keystore error is an error, so a hiccup never throws away sign-ins that still decrypt. The key is fetched once at startup on a background thread.
- **Sign-in via deep link** (Microsoft, built-in app only): the redirect is `com.peetsoftware.peetmail://auth`. The authorize page opens in a Custom Tab, and the redirect brings PeetMail back to the front (and closes the tab) before the token exchange, so the network is allowed. Your own app and Gmail keep the loopback redirect with the offline retry. **Not yet tried end to end:** the redirect URI has to be added to the built-in Entra registration first (Authentication → Mobile and desktop applications → `com.peetsoftware.peetmail://auth`). Until it is, Microsoft sign-in on Android shows Entra's redirect-mismatch error.
- **Attachments**: a tap opens one with `ACTION_VIEW` on a `FileProvider` URI from the cache; Android asks which app if there are several, and says so if there's none. The button beside it saves to Downloads through MediaStore (no permission needed). On a phone the chip has both, since there's no double-click.
- **Composer attachments**: the paperclip's `<input type=file>` already opens the system picker through wry. Android's WebView can't pass a raw request body over IPC, so Tauri sends the bytes as a JSON array of numbers; `add_draft_file` takes either.
- **Share → PeetMail**: `SEND` / `SEND_MULTIPLE` with any type. Files are copied into the cache while the sharing app's permission lasts, then added as draft attachments in Rust, and the composer opens with them, the subject and the text. Files PeetMail isn't allowed to read show as "Couldn't attach".
- **Notifications**: the permission is asked once, when an account is signed in and notifications are on, and again when they're turned on in Settings. Settings shows "Blocked by Android" with a button to its notification settings. Each single-message notification carries its account and conversation in `extra`, and a tap opens it, warm or from a cold start (checked by sending the same intent the notification plugin builds).
- **Diagnostics**: Export writes the zip to the cache and opens the share sheet.
- `docs/user/android.md`, shown in Help on Android only until the app is released.

Not done:
- Snooze and send-later reminders as scheduled notifications that fire with the app closed. Reminders still come from the sync loop; Stage 5's worker is where they belong.
- Gmail sign-in on Android is still untried.
- The status bar text is dark over the app's dark chrome on some screens (the composer, Settings). It needs `WindowInsetsController` light/dark icons following the theme.

## Stage 5: background sync (required for v1)

- A WorkManager periodic job (15 min, network required) that starts the Rust core headless and runs one `outbox::kick` + `mail::sync_all`, then raises new-mail notifications.
- Needs a refactor first: `sync_all` and the outbox take an `AppHandle` today. Split out a core context (`Db`, HTTP clients, an event sink) that the Tauri app and the worker both build, so the worker doesn't need a webview.
- Must coexist with a foreground app process (the database is already one connection behind a mutex; the worker either skips when the app is open or goes through the same process).
- Battery: one delta request per folder per run, same as desktop. Settings gets "Check for mail: every 15 min / hourly / only when open".

## Stage 6: distribution

- Signing key kept like the updater key (`docs/SECRETS.md`); CI builds a signed APK on tags (arm64 only: no 32-bit phones run Android 17).
- Channel: the signed APK attached to each GitHub Release (AAB not needed). Canary and stable follow the desktop channels.
- Updates in the app: the release feed gains an `android` entry (APK URL, size, SHA-256, minisign signature from the existing updater key). PeetMail checks it as desktop does, downloads in the background, checks the signature, and opens the system installer (`REQUEST_INSTALL_PACKAGES`, a `FileProvider` URI). The first install needs "Install unknown apps" allowed for the browser; later ones for PeetMail itself. Covered by the fake update server in the harness tests on the Rust side.
- The APK signing key must never change: Android refuses an update signed by a different key, and the only way out is uninstalling, which loses local data.
- CI: an `android` job builds the APK, boots an emulator, installs it and runs a smoke test, like `tests/linux/smoke.sh`. Android WebView exposes CDP (`adb forward ... localabstract:webview_devtools_remote_<pid>`), so a later step can run a few Playwright specs on the emulator against fake Graph at `10.0.2.2`.
- `CHANGELOG.md` and the What's New dialog work as on desktop; `docs/RELEASING.md` gains the Android steps.

## Risks

| Risk | Plan |
|---|---|
| **Google sign-in** (preferred for v1, not required). Google ties OAuth client types to platforms. The Desktop client's loopback redirect may work from a phone technically, but Google's Android client type wants custom schemes off and the Credential Manager / Authorization API, which doesn't hand a refresh token to a public client. | Spike in Stage 0: try the existing Desktop client over loopback. If Google blocks or flags it, v1 ships Microsoft only and Gmail accounts show as "not available on Android yet" (including ones arriving through settings sync). |
| Email rendering at 360–400 px | The fit-to-width step in Stage 2 is the main readability risk; build it early with a corpus of real newsletters in the fake Graph fixtures. |
| Android WebView versions | Updated through the Play Store, so it can lag. Less of a worry at Android 17, but still show a "please update Android System WebView" page below a minimum Chrome version. |
| `+page.svelte` growth | Already 487 lines of keymap and layout. Splitting the shells keeps desktop changes from breaking compact and vice versa. |
| E2E is Windows-only | Stage 2/3 tests run the compact layout on Windows, which covers layout and logic but not Android's WebView, keyboard or lifecycle. The emulator job in Stage 6 closes some of the gap; the rest is checked by hand per release. |
| Background limits | Android 17 can defer WorkManager jobs (Doze, app standby buckets, battery saver), so "every 15 min" is a best case. Show the last sync time on the list; settings links to the system battery setting to let PeetMail run unrestricted. |
| Process death | Android can kill PeetMail any time it's in the background. The outbox is already durable in SQLite and drafts save as you type, so nothing should be lost; test it with "Don't keep activities". |

## Open questions

1. Android 17 is very recent; does your phone run it? If you meant API level 17 (Android 4.2): Tauri needs API 24 at least.
