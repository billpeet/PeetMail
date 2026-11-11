# PeetMail

Keyboard-first mail + calendar client for Windows and Linux: Tauri 2 (Rust core) + SvelteKit SPA. Architecture, roadmap and decisions: `docs/PLAN.md`.

## Verify every change

Run the narrowest check that covers the change, then widen:

1. `pnpm check` for any TS or Svelte edit; `pnpm server:check` and `pnpm server:test` for anything in `server/` (the self-hosted sync server, `server/README.md`).
2. `pnpm test:unit` (Vitest, `src/**/*.test.ts`) and `pnpm test:rust` (`cargo test`) for logic changes.
3. `pnpm test:e2e` for anything a user can see or that touches sync, auth or IPC. Playwright drives the real app against a fake Microsoft Graph, headless.
4. For UI work, look at it: `pnpm drive start`, drive it with keys, `pnpm drive shot`, and read the PNG.

Done means `pnpm verify` is green and every new behaviour has a test that would go red without it. Testing details, the drive CLI and the fake server: `tests/README.md`.

## Gotchas

- The harness runs its own `tauri build --debug` exe from `src-tauri/target-harness` (separate from `target/`, so a running `pnpm tauri dev` never locks it). It rebuilds automatically via a stamp file. A plain `cargo build` exe loads the Vite dev server instead of the embedded frontend, so it is never used for tests.
- Test-only env overrides (`PEETMAIL_*`, see `src-tauri/src/testenv.rs`) are compiled out of release builds. Keep new test knobs there.
- Tests and the drive CLI run in their own data dir and WebView2 profile. The user's real data lives in `%APPDATA%\com.peetsoftware.peetmail`; leave it alone.
- `pnpm tauri dev` (any build that loads the Vite server, `tauri::is_dev()`) keeps its data in `.peetmail-dev/` (gitignored) in its own checkout, so each worktree has its own. Its first run copies the installed app's database there (minus queued outbox work) and migrates the copy, so the installed version keeps opening its own. Delete `.peetmail-dev/` to take a fresh copy.
- UI elements that tests or agents target get a `data-testid`. Add one with every new interactive element.
- Every action is a binding in the keymap registry (`src/lib/keys.svelte.ts`) so it shows in the `?` overlay and the future command palette.
- Clippy runs with `-D warnings` (`pnpm lint:rust`, part of `pnpm verify` and CI). The Rust code isn't rustfmt-formatted; don't reformat files wholesale.
- The version lives in `package.json`; `pnpm version:set <x.y.z>` copies it to `Cargo.toml` and `Cargo.lock`. Releasing: `docs/RELEASING.md`.
- Harness builds merge `src-tauri/tauri.harness.conf.json` over `tauri.conf.json`: the updater trusts the test key in `tests/harness/updater/` and allows the fake update server's http. Never put test keys or `dangerous*` flags in `tauri.conf.json`.
- Telemetry and crash reports must not carry personal data. Span and metric attributes go through `telemetry::attr` with a `telemetry::Key` (a test rejects anything else); free text that leaves the PC goes through `scrub::scrub`. The E2E canary tests (`tests/e2e/telemetry.spec.ts`, `sentry.spec.ts`) fail on any leak.
- `src-tauri/icons/` is generated: edit the SVGs in `src-tauri/icons/source/` and run `pnpm icons` (needs Edge, which Windows has).
- Every change a user can notice adds a line under `## Unreleased` in `CHANGELOG.md`, in the same commit, written for users (what changed for them, not how). The app shows these lines after an update, and a stable release fails without its section.
- Linux is a supported platform (an AppImage, built on Ubuntu 22.04 for glibc reach). Platform code goes behind `#[cfg(windows)]` / `#[cfg(target_os = "linux")]` with both arms present; `windows-sys` is a Windows-only dependency. `pnpm lint:rust` on Windows doesn't compile the Linux arms: check them with clippy and `cargo test` on Linux (CI's `linux` job, WSL, or a container; `tests/README.md` → Linux). The E2E suite is Windows-only (it drives WebView2 over CDP); on Linux CI builds an AppImage and runs `tests/linux/smoke.sh`.
- Android (`docs/ANDROID.md`) is a third arm, not a fallback: Android is `unix` but not `target_os = "linux"`, so code gated on Linux alone doesn't reach it. Use `#[cfg(desktop)]` / `#[cfg(mobile)]` for Tauri features (tray, window, updater, autostart) and `target_os = "android"` for OS code. `pnpm lint:android` runs clippy and a test build for `aarch64-linux-android` (needs the NDK; CI's `android` job). Android 17 cuts a backgrounded app off the network: anything that runs while PeetMail isn't in front goes through `lifecycle.rs`.
- `docs/user/` is the help for people using PeetMail, compiled into the app's Help (F1). Change it in the same commit as the behaviour it describes; a unit test checks every page exists, has a title and that its links resolve.
