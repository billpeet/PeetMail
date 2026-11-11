// Launches the real PeetMail binary against the fake Graph server, isolated from
// the user's real data, and connects Playwright to its WebView2 over CDP.
// Used by the Playwright fixtures (tests/e2e) and the drive CLI.

import { spawn, spawnSync } from "node:child_process";
import { createWriteStream, existsSync, mkdirSync, readFileSync, statSync, readdirSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { spawn as spawnType } from "node:child_process";
import { chromium, type Browser, type Page } from "@playwright/test";

/** What the launcher needs from a fake server (in-process or a detached one). */
export type FakeEndpoints = { graphUrl: string; authorityUrl: string };

export type LaunchOptions = {
  fake: FakeEndpoints;
  dataDir?: string;
  cdpPort?: number;
  syncIntervalSecs?: number;
  /** Undo-send delay. 0 (the default here) sends at once, so tests don't wait out the app's 10s. */
  undoSendSecs?: number;
  /** More environment for the app (e.g. the fake Gmail's URLs). */
  extraEnv?: Record<string, string>;
  headed?: boolean;
  /** Run `pnpm tauri dev` (hot reload) instead of the prebuilt debug exe. */
  dev?: boolean;
  /** Leave the app running after this process exits (used by the drive CLI). */
  detached?: boolean;
  /** Window and page size instead of TEST_VIEWPORT; PHONE_VIEWPORT gives the compact layout. */
  viewport?: { width: number; height: number };
};

export type LaunchedApp = {
  page: Page;
  browser: Browser;
  proc: ReturnType<typeof spawnType>;
  dataDir: string;
  cdpPort: number;
  logPath: string;
  close(): Promise<void>;
};

export const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
/**
 * The harness builds into its own target dir, so tests never fight a dev copy of the app
 * (`pnpm tauri dev`) over a locked target/debug/peetmail.exe.
 */
const TARGET_DIR = join(ROOT, "src-tauri/target-harness");
export const EXE = join(TARGET_DIR, "debug/peetmail.exe");
/**
 * Written after a successful `tauri build --debug`. A plain `cargo build` produces an
 * exe that loads the Vite dev server instead of the embedded frontend, so an exe newer
 * than the stamp means "not built by us" and forces a rebuild.
 */
const STAMP = join(TARGET_DIR, "debug/.harness-build-stamp");

export function freePort(): Promise<number> {
  return new Promise((res, rej) => {
    const s = createServer();
    s.unref();
    s.on("error", rej);
    s.listen(0, "127.0.0.1", () => {
      const { port } = s.address() as import("node:net").AddressInfo;
      s.close(() => res(port));
    });
  });
}

function newestMtime(dir: string): number {
  let newest = 0;
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    newest = Math.max(newest, entry.isDirectory() ? newestMtime(p) : statSync(p).mtimeMs);
  }
  return newest;
}

/** True when the debug exe is missing or older than any frontend/Rust source. */
export function needsBuild(): boolean {
  if (!existsSync(EXE) || !existsSync(STAMP)) return true;
  const built = statSync(STAMP).mtimeMs;
  if (statSync(EXE).mtimeMs > built + 1000) return true;
  const sources = ["src", "src-tauri/src", "src-tauri/migrations", "src-tauri/icons", "docs/user", "static"].map((d) => join(ROOT, d)).filter(existsSync);
  // CHANGELOG.md is compiled into the app (What's new).
  const files = ["src-tauri/tauri.conf.json", HARNESS_CONFIG, "CHANGELOG.md", "src-tauri/Cargo.toml", "package.json", "vite.config.js"].map((f) => join(ROOT, f));
  return Math.max(...sources.map(newestMtime), ...files.filter(existsSync).map((f) => statSync(f).mtimeMs)) > built;
}

/**
 * Merged over tauri.conf.json for every harness build: the updater trusts the test signing
 * key (tests/harness/updater) and accepts the fake update server's plain http.
 */
const HARNESS_CONFIG = "src-tauri/tauri.harness.conf.json";

/** Builds the debug binary (frontend embedded) if sources changed. Set PEETMAIL_SKIP_BUILD=1 to skip. */
export function ensureBuilt({ force = false, log = console.log }: { force?: boolean; log?: (msg: string) => void } = {}) {
  if (process.env.PEETMAIL_SKIP_BUILD === "1" && existsSync(STAMP)) return;
  if (!force && !needsBuild()) return;
  log("building debug app (pnpm tauri build --debug --no-bundle)…");
  const r = spawnSync(`pnpm tauri build --debug --no-bundle --config ${HARNESS_CONFIG}`, {
    cwd: ROOT,
    stdio: "inherit",
    shell: true,
    env: { ...process.env, CARGO_TARGET_DIR: TARGET_DIR },
  });
  if (r.status !== 0) throw new Error("debug build failed");
  writeFileSync(STAMP, new Date().toISOString());
}

/**
 * The page size tests see: the app window's own size (1280x820 less the frame), just wide
 * enough for the right pane (shown from 1280px). Fixed rather than taken from the window,
 * which Windows shrinks to fit a small screen (CI runners have 1024x768).
 */
export const TEST_VIEWPORT = { width: 1280, height: 792 };

/** A phone (a Pixel 8's CSS pixels): narrower than COMPACT_MAX_WIDTH, so the compact layout. */
export const PHONE_VIEWPORT = { width: 412, height: 915 };

/** Headless unless PEETMAIL_HEADED=1 (or `headed: true`), so test windows never interrupt you. */
export const DEFAULT_HEADED = process.env.PEETMAIL_HEADED === "1";

// The app passes these to WebView2 (PEETMAIL_WEBVIEW2_ARGS), replacing Tauri's own, so
// those are repeated: no mini menu or SmartScreen, autoplay allowed.
const TAURI_DEFAULT_FEATURES = "msWebOOUI,msPdfOOUI,msSmartScreenProtection";
const BASE_BROWSER_ARGS = "--autoplay-policy=no-user-gesture-required";
// Off-screen windows count as occluded to Chromium, which then throttles rendering and timers.
const HEADLESS_BROWSER_ARGS = [
  "--disable-backgrounding-occluded-windows",
  "--disable-renderer-backgrounding",
  "--disable-background-timer-throttling",
].join(" ");

function browserArgs(cdpPort: number, headed: boolean): string {
  const features = headed ? TAURI_DEFAULT_FEATURES : `${TAURI_DEFAULT_FEATURES},CalculateNativeWinOcclusion`;
  return [`--remote-debugging-port=${cdpPort}`, `--disable-features=${features}`, BASE_BROWSER_ARGS, ...(headed ? [] : [HEADLESS_BROWSER_ARGS])].join(" ");
}

export function testEnv({
  fake,
  dataDir,
  cdpPort,
  syncIntervalSecs = 3600,
  undoSendSecs = 0,
  headed = DEFAULT_HEADED,
}: { fake: FakeEndpoints; dataDir: string; cdpPort: number; syncIntervalSecs?: number; undoSendSecs?: number; headed?: boolean }): Record<string, string> {
  return {
    ...(headed ? {} : { PEETMAIL_HEADLESS: "1" }),
    PEETMAIL_DATA_DIR: dataDir,
    PEETMAIL_GRAPH_URL: fake.graphUrl,
    PEETMAIL_AUTHORITY_URL: fake.authorityUrl,
    PEETMAIL_AUTH_AUTOFOLLOW: "1",
    PEETMAIL_SYNC_INTERVAL_SECS: String(syncIntervalSecs),
    PEETMAIL_UNDO_SEND_SECS: String(undoSendSecs),
    // Toasts go to a file the tests read; "start with Windows" never touches the registry.
    PEETMAIL_NOTIFY_LOG: join(dataDir, "notifications.jsonl"),
    PEETMAIL_FAKE_AUTOSTART: "1",
    // Links the app opens are written here instead of opening a browser on this machine.
    PEETMAIL_OPEN_LOG: join(dataDir, "opened.txt"),
    PEETMAIL_DOWNLOAD_DIR: join(dataDir, "downloads"),
    // Own WebView2 profile, so a running copy of the real app can't collide with tests.
    WEBVIEW2_USER_DATA_FOLDER: join(dataDir, "webview2"),
    PEETMAIL_WEBVIEW2_ARGS: browserArgs(cdpPort, headed),
  };
}

export async function waitForCdp(
  port: number,
  { timeoutMs = 30_000, proc }: { timeoutMs?: number; proc?: ReturnType<typeof spawnType> } = {},
) {
  const deadline = Date.now() + timeoutMs;
  // What the port last said, for the error: nothing listening, or which targets.
  let seen = "nothing listening";
  while (Date.now() < deadline) {
    if (proc && proc.exitCode !== null) throw new Error(`app exited with code ${proc.exitCode} before WebView2 came up`);
    try {
      const targets: { type: string; url: string }[] = await (await fetch(`http://127.0.0.1:${port}/json`)).json();
      if (targets.some((t) => t.type === "page" && /tauri\.localhost|localhost:1420/.test(t.url))) return;
      seen = `targets: ${JSON.stringify(targets.map((t) => `${t.type} ${t.url}`))}`;
    } catch {}
    await new Promise((r) => setTimeout(r, 200));
  }
  throw new Error(`WebView2 DevTools endpoint on port ${port} did not come up within ${timeoutMs}ms (${seen})`);
}

/** The end of a text file, or a note that it isn't there. */
function tail(path: string, lines = 30): string {
  try {
    const all = readFileSync(path, "utf8").split(/\r?\n/);
    // A Rust panic's message comes before a long backtrace: show it, not just the end.
    const panic = all.findIndex((l) => l.includes("panicked at"));
    const head = panic >= 0 && panic < all.length - lines ? [...all.slice(panic, panic + 4), "..."] : [];
    return [...head, ...all.slice(-lines)].join("\n").trim() || "(empty)";
  } catch {
    return "(missing)";
  }
}

/** Everything that might say why the app didn't come up (shown in the CI log). */
function launchDiagnostics(dataDir: string, logPath: string, proc: ReturnType<typeof spawnType>): string {
  const logsDir = join(dataDir, "logs");
  const appLogs = existsSync(logsDir) ? readdirSync(logsDir).map((f) => `--- ${f}\n${tail(join(logsDir, f))}`).join("\n") : "(no logs dir)";
  const profile = join(dataDir, "webview2");
  return [
    `exe: ${EXE} (exists: ${existsSync(EXE)}), pid ${proc.pid}, still running: ${proc.exitCode === null}`,
    `WebView2 profile created: ${existsSync(profile)}`,
    `--- stdout/stderr (${logPath})\n${tail(logPath)}`,
    appLogs,
    webview2Diagnostics(profile),
  ].join("\n");
}

/**
 * Whether WebView2 got (and honoured) --remote-debugging-port: the profile's contents, the
 * port file it writes when debugging is on, the browser processes' command lines, and any
 * Edge/WebView2 policies (a machine policy can switch remote debugging off).
 */
function webview2Diagnostics(profile: string): string {
  const out: string[] = [];
  const list = (dir: string, depth: number, indent = ""): void => {
    if (depth < 0 || !existsSync(dir)) return;
    for (const f of readdirSync(dir).slice(0, 25)) {
      out.push(`${indent}${f}`);
      try {
        if (statSync(join(dir, f)).isDirectory()) list(join(dir, f), depth - 1, indent + "  ");
      } catch {}
    }
  };
  out.push("--- WebView2 profile");
  list(profile, 1);
  const portFile = join(profile, "EBWebView", "DevToolsActivePort");
  out.push(`DevToolsActivePort: ${existsSync(portFile) ? readFileSync(portFile, "utf8").replace(/\s+/g, " ") : "(none: debugging not on)"}`);
  const ps = (cmd: string) =>
    spawnSync("powershell", ["-NoProfile", "-Command", cmd], { encoding: "utf8", timeout: 15_000 }).stdout?.trim() || "(none)";
  const name = profile.replace(/'/g, "''");
  out.push(
    "--- msedgewebview2 processes for this profile",
    ps(
      `Get-CimInstance Win32_Process -Filter "Name='msedgewebview2.exe'" | Where-Object { $_.CommandLine -like '*${name}*' -and $_.CommandLine -notlike '*--type=*' } | ForEach-Object { $_.CommandLine.Substring(0, [Math]::Min(1500, $_.CommandLine.Length)) }`,
    ),
    "--- Edge / WebView2 policies",
    ps(`Get-ChildItem -Recurse 'HKLM:\\SOFTWARE\\Policies\\Microsoft\\Edge','HKCU:\\SOFTWARE\\Policies\\Microsoft\\Edge' -ErrorAction SilentlyContinue | ForEach-Object { $_.Name; ($_ | Get-ItemProperty | Out-String).Trim() }`),
  );
  return out.join("\n");
}

export async function connect(cdpPort: number): Promise<{ browser: Browser; page: Page }> {
  // The DevTools target list shows the app's URL as soon as WebView2 starts loading it;
  // Playwright shows the page once that navigation commits. Locally that's well under a
  // second; on a loaded CI runner it has taken over 15s (attaching again didn't help: the
  // page really was still on about:blank). So wait, and say when it was slow.
  const isApp = (p: Page) => /tauri\.localhost|localhost:1420/.test(p.url());
  const started = Date.now();
  const browser = await chromium.connectOverCDP(`http://127.0.0.1:${cdpPort}`);
  const deadline = started + 45_000;
  while (Date.now() < deadline) {
    const page = browser.contexts().flatMap((c) => c.pages()).find(isApp);
    if (page) {
      const took = Date.now() - started;
      if (took > 5_000) console.warn(`[harness] the app page took ${(took / 1000).toFixed(1)}s to load`);
      return { browser, page };
    }
    await new Promise((r) => setTimeout(r, 100));
  }
  const seen = browser.contexts().flatMap((c) => c.pages()).map((p) => p.url());
  await browser.close().catch(() => {});
  throw new Error(`PeetMail page not found over CDP within 45s (pages: ${JSON.stringify(seen)})`);
}

export function killTree(pid: number | undefined) {
  if (!pid) return;
  spawnSync("taskkill", ["/PID", String(pid), "/T", "/F"], { stdio: "ignore" });
}

/**
 * Starts the app. Returns { page, browser, proc, dataDir, cdpPort, logPath, close }.
 * `dev: true` runs `pnpm tauri dev` (hot reload) instead of the prebuilt exe.
 */
/** Prefix of the temporary data dirs tests create (one per app launch). */
export const TEMP_PREFIX = "peetmail-test-";

/**
 * Deletes a data dir, retrying while WebView2's processes (which outlive the app by a
 * moment) still hold files in it. Each dir holds a WebView2 profile (~10 MB); thousands
 * of leftovers once filled the disk.
 */
export async function removeDataDir(dir: string, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    try {
      await rm(dir, { recursive: true, force: true });
      return;
    } catch (e) {
      if (Date.now() > deadline) {
        console.warn(`couldn't remove ${dir}: ${e}`);
        return;
      }
      await new Promise((r) => setTimeout(r, 250));
    }
  }
}

/** Removes temp data dirs left by earlier runs (crashed or killed before cleaning up). */
export async function sweepStaleDataDirs(olderThanMs = 15 * 60_000) {
  const now = Date.now();
  const stale = readdirSync(tmpdir()).filter((n) => n.startsWith(TEMP_PREFIX)).map((n) => join(tmpdir(), n))
    .filter((p) => {
      try {
        return now - statSync(p).mtimeMs > olderThanMs;
      } catch {
        return false;
      }
    });
  await Promise.all(stale.map((p) => removeDataDir(p, 2_000)));
  return stale.length;
}

export async function launchApp({ fake, dataDir, cdpPort, syncIntervalSecs, undoSendSecs, extraEnv, headed, dev = false, detached = false, viewport }: LaunchOptions): Promise<LaunchedApp> {
  if (viewport) extraEnv = { ...extraEnv, PEETMAIL_WINDOW_SIZE: `${viewport.width}x${viewport.height}` };
  // A temp dir we create is ours to delete; one passed in (the drive CLI's) is kept.
  const ownsDataDir = !dataDir && !detached;
  dataDir ??= await mkdtemp(join(tmpdir(), TEMP_PREFIX));
  mkdirSync(dataDir, { recursive: true });
  cdpPort ??= await freePort();
  const logPath = join(dataDir, "app-stdout.log");
  const env = { ...process.env, ...testEnv({ fake, dataDir, cdpPort, syncIntervalSecs, undoSendSecs, headed }), ...extraEnv };

  const out = detached ? "ignore" : createWriteStream(logPath);
  const proc = dev
    ? spawn(`pnpm tauri dev --config ${HARNESS_CONFIG}`, { cwd: ROOT, env, shell: true, detached, stdio: detached ? "ignore" : ["ignore", "pipe", "pipe"] })
    : spawn(EXE, [], { cwd: ROOT, env, detached, stdio: detached ? "ignore" : ["ignore", "pipe", "pipe"] });
  if (!detached && typeof out !== "string") {
    proc.stdout?.pipe(out);
    proc.stderr?.pipe(out);
  } else {
    proc.unref();
  }

  try {
    await waitForCdp(cdpPort, { timeoutMs: dev ? 300_000 : 30_000, proc: detached ? undefined : proc });
  } catch (e) {
    const why = detached ? "" : `\n${launchDiagnostics(dataDir, logPath, proc)}`;
    killTree(proc.pid);
    throw new Error(`${e instanceof Error ? e.message : String(e)}${why}`);
  }
  const { browser, page } = await connect(cdpPort);
  // A hidden test window: same layout on any screen. A visible one keeps its real size.
  if (viewport) await page.setViewportSize(viewport);
  else if (!(headed ?? DEFAULT_HEADED)) await page.setViewportSize(TEST_VIEWPORT);
  await page.waitForLoadState("domcontentloaded");
  await page.locator("body").waitFor();
  await waitForKeyboard(page);

  return {
    page,
    browser,
    proc,
    dataDir,
    cdpPort,
    logPath,
    async close() {
      await browser.close().catch(() => {});
      killTree(proc.pid);
      if (ownsDataDir) await removeDataDir(dataDir!);
    },
  };
}

/**
 * On the first launches of a freshly built exe, WebView2 silently drops keys sent over
 * DevTools for a while after the page loads (no keydown ever reaches the page). Press a
 * harmless key until the page sees it, so tests never type into the void.
 */
export async function waitForKeyboard(page: Page, timeoutMs = 20_000) {
  await page.evaluate(() => {
    const w = window as unknown as { __keyProbe?: boolean };
    w.__keyProbe = false;
    window.addEventListener("keydown", () => (w.__keyProbe = true), { capture: true, once: true });
  });
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    await page.keyboard.press("Shift"); // modifiers alone are ignored by the keymap
    if (await page.evaluate(() => (window as unknown as { __keyProbe?: boolean }).__keyProbe)) {
      return;
    }
    await page.waitForTimeout(100);
  }
  throw new Error(`WebView2 never delivered keyboard input within ${timeoutMs}ms`);
}

/** What the app is showing right now, read from data-testid hooks. Used by drive and in error messages. */
export function screenState(page: Page) {
  return page.evaluate(() => {
    const q = (s: string) => document.querySelector<HTMLElement>(s);
    const sel = q("[data-testid=thread-row][aria-selected=true]");
    return {
      view: q("[data-testid=setup]") ? "setup" : q("[data-testid=reader]") ? "reader" : q("[data-testid=thread-list]") ? "list" : "unknown",
      overlay: q("[data-testid=settings]") ? "settings" : q("[data-testid=shortcut-help]") ? "help" : null,
      selected: sel ? { conversationId: sel.dataset.conversationId, text: sel.innerText.replace(/\s+/g, " ").slice(0, 100) } : null,
      readerSubject: q("[data-testid=reader] h1")?.textContent ?? null,
      threadCount: q("[data-testid=thread-count]")?.textContent ?? null,
      sync: q("[data-testid=sync-status]")?.dataset.state ?? null,
      clientIdInput: (q("[data-testid=client-id-input]") as HTMLInputElement | null)?.value ?? null,
      error: q("[data-testid=app-error]")?.textContent ?? q("[data-testid=setup-error]")?.textContent ?? q("[data-testid=client-id-error]")?.textContent ?? null,
      focused: (document.activeElement as HTMLElement | null)?.dataset?.testid ?? document.activeElement?.tagName ?? null,
    };
  });
}

/** Waits for `locator`; on timeout the error includes the current screen state. */
async function waitOrExplain(page: Page, locator: ReturnType<Page["getByTestId"]>, what: string, timeout: number) {
  try {
    await locator.waitFor({ timeout });
  } catch {
    const state = await screenState(page).catch((e) => `unavailable: ${e}`);
    throw new Error(`timed out after ${timeout}ms waiting for ${what}; screen: ${JSON.stringify(state)}`);
  }
}

/** Drives the setup screen: enters the fake client ID (if asked) and signs in. */
export async function signIn(page: Page, clientId: string) {
  const input = page.getByTestId("client-id-input");
  const signInButton = page.getByTestId("sign-in");
  await waitOrExplain(page, input.or(signInButton).first(), "the setup screen", 15_000);
  if (await input.isVisible()) {
    await input.fill(clientId);
    await input.press("Enter");
  }
  await waitOrExplain(page, signInButton, "the sign-in button", 15_000);
  await page.keyboard.press("Enter");
  await waitOrExplain(page, page.getByTestId("inbox"), "the inbox after sign-in", 15_000);
}
