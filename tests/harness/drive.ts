#!/usr/bin/env node
// Step-by-step driver for a running PeetMail test instance. Built for agents:
// every command is one shell call, prints plain text, and exits non-zero on failure.
//
//   pnpm drive start            fake Graph + headless app, signed in, first sync done
//   pnpm drive key j j Enter    press keys (Playwright key names; "ctrl+r" also works)
//   pnpm drive shot             screenshot -> prints the PNG path
//   pnpm drive stop
//
// Run `pnpm drive help` for everything. State lives in .peetmail-test/ (gitignored).

import { spawn } from "node:child_process";
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync, openSync } from "node:fs";
import { join, resolve } from "node:path";
import type { Page } from "@playwright/test";
import { connect, ensureBuilt, freePort, killTree, launchApp, ROOT, screenState, signIn, PHONE_VIEWPORT } from "./app.ts";
import { CLIENT_ID } from "./fake-graph.ts";

const DIR = join(ROOT, ".peetmail-test");
const SESSION = join(DIR, "session.json");
const SHOTS = join(DIR, "shots");

const HELP = `pnpm drive <command> [args]

Session
  start [--fresh] [--headed] [--dev] [--phone] [--count N] [--seed N]
        Start the fake Graph server and the app (headless unless --headed).
        Signs in and waits for the first sync unless --fresh (stays on setup screen).
        --dev runs \`pnpm tauri dev\` (hot reload) instead of the prebuilt debug exe.
        --phone makes the window phone-sized (412x915): the compact layout (docs/ANDROID.md).
  stop [--clean]       Stop app and fake server (--clean also deletes the data dir)
  restart              Restart only the app, keeping its data (tests persistence)
  status               Session info plus current screen state

Look
  shot [file] [--selector CSS] [--full]   Screenshot; prints the PNG path
  state                Current view, selection, overlay, sync state as JSON
  text [selector]      innerText of the page or an element
  body [n]             innerText of the nth open message body iframe (default 0)
  eval <js>            Evaluate a JS expression in the page, print JSON result
  logs [--lines N]     Tail the app log

Act
  key <key...>         Press keys in order: j, Enter, Escape, Shift+G, "ctrl+r", "?"
  type <text>          Type text into the focused element
  click <selector>     Click (Playwright selector: CSS, text=..., [data-testid=...])
  wait <selector> [--hidden] [--timeout ms]

Fake server
  graph <route> [json] Call the control API, e.g.
                         graph deliver '{"subject":"Hi","count":3}'
                         graph update '{"id":"msg-hostile","fields":{"isRead":true}}'
                         graph throttle '{"count":2}'
                         graph requests
                       Routes: GET /_test on the fake server lists them all.

Useful selectors: [data-testid=thread-row] (data-conversation-id, aria-selected,
data-unread), [data-testid=reader], [data-testid=message], [data-testid=sync-status]
(data-state), [data-testid=client-id-input], [data-testid=sign-in].`;

// ---------- session ----------

type Session = {
  fakePid: number;
  fakePort: number;
  appPid: number;
  cdpPort: number;
  dataDir: string;
  headed: boolean;
  dev: boolean;
  startedAt: string;
  /** `start --phone`: a phone-sized window, the compact layout. */
  phone?: boolean;
};

function loadSession(): Session {
  if (!existsSync(SESSION)) fail("no running session; run `pnpm drive start`");
  return JSON.parse(readFileSync(SESSION, "utf8"));
}

function alive(pid: number | undefined): boolean {
  if (!pid) return false;
  try {
    process.kill(pid, 0);
    return true;
  } catch {
    return false;
  }
}

function fail(msg: string): never {
  console.error(`error: ${msg}`);
  process.exit(1);
}

function flag(args: string[], name: string): boolean {
  const i = args.indexOf(name);
  if (i < 0) return false;
  args.splice(i, 1);
  return true;
}

function option(args: string[], name: string, fallback?: string): string | undefined {
  const i = args.indexOf(name);
  if (i < 0) return fallback;
  const v = args[i + 1];
  args.splice(i, 2);
  return v;
}

async function waitForHttp(url: string, timeoutMs = 10_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if ((await fetch(url)).ok) return;
    } catch {}
    await new Promise((r) => setTimeout(r, 100));
  }
  throw new Error(`${url} did not respond`);
}

async function withPage<T>(fn: (page: Page, s: Session) => Promise<T>): Promise<T> {
  const s = loadSession();
  const { browser, page } = await connect(s.cdpPort);
  try {
    return await fn(page, s);
  } finally {
    // Disconnects only; the app keeps running.
    await browser.close().catch(() => {});
  }
}

async function printState(page: Page) {
  await page.waitForTimeout(150); // let Svelte flush
  console.log(JSON.stringify(await screenState(page)));
}

const KEY_ALIASES: Record<string, string> = { esc: "Escape", enter: "Enter", up: "ArrowUp", down: "ArrowDown", space: "Space", tab: "Tab" };
function normalizeKey(k: string): string {
  if (KEY_ALIASES[k.toLowerCase()]) return KEY_ALIASES[k.toLowerCase()];
  // Every modifier, not just the first: "ctrl+shift+b" -> "Control+Shift+b".
  const MODS: Record<string, string> = { ctrl: "Control", control: "Control", alt: "Alt", shift: "Shift", meta: "Meta" };
  const parts = k.split("+");
  return parts.map((p, i) => (i < parts.length - 1 ? (MODS[p.toLowerCase()] ?? p) : p)).join("+");
}

// ---------- commands ----------

const commands: Record<string, (args: string[]) => Promise<void>> = {
  help: async () => console.log(HELP),

  async start(args: string[]) {
    if (existsSync(SESSION)) {
      const s = JSON.parse(readFileSync(SESSION, "utf8"));
      if (alive(s.appPid) || alive(s.fakePid)) fail("a session is already running; `pnpm drive stop` first");
    }
    const fresh = flag(args, "--fresh");
    const headed = flag(args, "--headed");
    const dev = flag(args, "--dev");
    const phone = flag(args, "--phone");
    const count = option(args, "--count");
    const seed = option(args, "--seed");

    mkdirSync(DIR, { recursive: true });
    if (!dev) ensureBuilt({ log: (m) => console.log(m) });

    const fakePort = await freePort();
    const fakeArgs = [join(ROOT, "tests/harness/fake-graph.ts"), "--port", String(fakePort), "--verbose"];
    if (count) fakeArgs.push("--count", count);
    if (seed) fakeArgs.push("--seed", seed);
    const fakeLog = openSync(join(DIR, "fake-graph.log"), "w");
    const fakeProc = spawn(process.execPath, fakeArgs, { detached: true, stdio: ["ignore", fakeLog, fakeLog] });
    fakeProc.unref();
    const origin = `http://127.0.0.1:${fakePort}`;
    await waitForHttp(`${origin}/_test`);
    const fake = { origin, graphUrl: `${origin}/v1.0`, authorityUrl: `${origin}/oauth` };

    const dataDir = join(DIR, "data");
    rmSync(dataDir, { recursive: true, force: true });
    const app = await launchApp({ fake, dataDir, headed, dev, detached: true, viewport: phone ? PHONE_VIEWPORT : undefined });
    const session = { fakePid: fakeProc.pid, fakePort, appPid: app.proc.pid, cdpPort: app.cdpPort, dataDir, headed, dev, phone, startedAt: new Date().toISOString() };
    writeFileSync(SESSION, JSON.stringify(session, null, 2));

    try {
      if (!fresh) {
        await signIn(app.page, CLIENT_ID);
        await app.page.waitForFunction(
          () => document.querySelector<HTMLElement>("[data-testid=sync-status]")?.dataset.state === "idle",
          null,
          { timeout: 30_000 },
        );
      }
      console.log(`started: app pid ${session.appPid} (cdp ${session.cdpPort}), fake graph ${origin}${headed ? "" : ", headless"}`);
      await printState(app.page);
    } finally {
      await app.browser.close().catch(() => {});
    }
  },

  async stop(args: string[]) {
    const clean = flag(args, "--clean");
    if (!existsSync(SESSION)) return console.log("nothing running");
    const s = JSON.parse(readFileSync(SESSION, "utf8"));
    killTree(s.appPid);
    killTree(s.fakePid);
    rmSync(SESSION, { force: true });
    if (clean) {
      // WebView2 child processes release the profile a moment after the app dies.
      for (let i = 0; ; i++) {
        try {
          rmSync(s.dataDir, { recursive: true, force: true });
          break;
        } catch (e) {
          if (i >= 20) throw e;
          await new Promise((r) => setTimeout(r, 250));
        }
      }
    }
    console.log("stopped");
  },

  async restart() {
    const s = loadSession();
    killTree(s.appPid);
    await new Promise((r) => setTimeout(r, 500));
    const origin = `http://127.0.0.1:${s.fakePort}`;
    const fake = { origin, graphUrl: `${origin}/v1.0`, authorityUrl: `${origin}/oauth` };
    const app = await launchApp({ fake, dataDir: s.dataDir, headed: s.headed, dev: s.dev, detached: true, viewport: s.phone ? PHONE_VIEWPORT : undefined });
    writeFileSync(SESSION, JSON.stringify({ ...s, appPid: app.proc.pid, cdpPort: app.cdpPort }, null, 2));
    console.log(`restarted: app pid ${app.proc.pid} (cdp ${app.cdpPort})`);
    await printState(app.page);
    await app.browser.close().catch(() => {});
  },

  async status() {
    const s = loadSession();
    console.log(JSON.stringify({ ...s, appAlive: alive(s.appPid), fakeAlive: alive(s.fakePid) }, null, 2));
    if (alive(s.appPid)) await withPage((page) => printState(page));
  },

  state: () => withPage((page) => printState(page)),

  async shot(args: string[]) {
    const selector = option(args, "--selector");
    const full = flag(args, "--full");
    mkdirSync(SHOTS, { recursive: true });
    const file = resolve(args[0] ?? join(SHOTS, `${new Date().toISOString().replace(/[:.]/g, "-")}.png`));
    await withPage(async (page: Page) => {
      await page.waitForTimeout(150);
      if (selector) await page.locator(selector).first().screenshot({ path: file });
      else await page.screenshot({ path: file, fullPage: full });
    });
    console.log(file);
  },

  text: (args: string[]) =>
    withPage(async (page: Page) => console.log(await page.locator(args[0] ?? "body").first().innerText())),

  body: (args: string[]) =>
    withPage(async (page: Page) => {
      const n = Number(args[0] ?? 0);
      const frame = page.frameLocator("[data-testid=message-body]").nth(n);
      console.log(await frame.locator("body").innerText({ timeout: 10_000 }));
    }),

  eval: (args: string[]) =>
    withPage(async (page: Page) => {
      if (!args.length) fail("usage: eval <js expression>");
      const result = await page.evaluate(`(async () => (${args.join(" ")}))()`);
      console.log(JSON.stringify(result, null, 2));
    }),

  async logs(args: string[]) {
    const s = loadSession();
    const n = Number(option(args, "--lines", "40"));
    const file = join(s.dataDir, "logs", "PeetMail.log");
    if (!existsSync(file)) return console.log("(no log yet)");
    console.log(readFileSync(file, "utf8").trimEnd().split("\n").slice(-n).join("\n"));
  },

  key: (args: string[]) =>
    withPage(async (page: Page) => {
      if (!args.length) fail("usage: key <key...>");
      for (const k of args) {
        await page.keyboard.press(normalizeKey(k));
        await page.waitForTimeout(40);
      }
      await printState(page);
    }),

  type: (args: string[]) =>
    withPage(async (page: Page) => {
      await page.keyboard.type(args.join(" "));
      await printState(page);
    }),

  click: (args: string[]) =>
    withPage(async (page: Page) => {
      if (!args.length) fail("usage: click <selector>");
      await page.locator(args.join(" ")).first().click({ timeout: 10_000 });
      await printState(page);
    }),

  wait: (args: string[]) =>
    withPage(async (page: Page) => {
      const hidden = flag(args, "--hidden");
      const timeout = Number(option(args, "--timeout", "10000"));
      await page.locator(args.join(" ")).first().waitFor({ state: hidden ? "hidden" : "visible", timeout });
      await printState(page);
    }),

  async graph(args: string[]) {
    const s = loadSession();
    const [route, json] = args;
    if (!route) fail("usage: graph <route> [json]");
    const isGet = !json && ["requests", "state", ""].includes(route);
    const res = await fetch(`http://127.0.0.1:${s.fakePort}/_test/${route}`, {
      method: isGet ? "GET" : "POST",
      headers: { "Content-Type": "application/json" },
      body: isGet ? undefined : (json ?? "{}"),
    });
    const text = await res.text();
    console.log(text);
    if (!res.ok) process.exit(1);
  },
};

const [cmd = "help", ...args] = process.argv.slice(2);
const run = commands[cmd];
if (!run) fail(`unknown command "${cmd}"\n\n${HELP}`);
try {
  await run(args);
} catch (e) {
  fail(e instanceof Error ? e.message : String(e));
}
