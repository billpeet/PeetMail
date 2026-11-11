// Playwright fixtures: every test gets a fresh fake Graph server and a fresh app
// instance with its own data dir and WebView2 profile.
//
//   fake   the in-process fake server (control API: fake.control("deliver", {...}))
//   app    launched app handle ({ page, dataDir, logPath, ... }), not signed in
//   page   the app's WebView2 page (overrides Playwright's built-in browser page)
//   inbox  the page after signing in and finishing the first sync
//
// After each test a final screenshot and the app log are attached to the report.

import { test as base, expect, type Page, type TestInfo } from "@playwright/test";
import { readFile } from "node:fs/promises";
import { startFakeGraph } from "../harness/fake-graph.ts";
import { startFakePair } from "../harness/fake-pair.ts";
import { startFakeGmail, GOOGLE_CLIENT_ID, GOOGLE_CLIENT_SECRET } from "../harness/fake-gmail.ts";
import { CLIENT_ID } from "../harness/fake-graph.ts";
import { startFakeUpdates, type FakeUpdates, type Release } from "../harness/fake-update.ts";
import { startFakeSentry, type FakeSentry } from "../harness/fake-sentry.ts";
import { startFakeOtlp, type FakeOtlp } from "../harness/fake-otlp.ts";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { rm } from "node:fs/promises";
import { launchApp, PHONE_VIEWPORT, signIn } from "../harness/app.ts";
import { defaultMailbox, FOLDER_IDS } from "../harness/fixtures.ts";

type Fake = Awaited<ReturnType<typeof startFakeGraph>>;
type Pair = Awaited<ReturnType<typeof startFakePair>>;
type Gmail = Awaited<ReturnType<typeof startFakeGmail>>;
type App = Awaited<ReturnType<typeof launchApp>>;
/** The fake update server, plus the file the app writes instead of running an installer. */
type Updates = FakeUpdates & { installedPath: string };

export const test = base.extend<{ fake: Fake; app: App; page: Page; inbox: Page; undoSendSecs: number; secondAccount: boolean; pair: Pair; withGmail: boolean; gmailExtra: number; gmail: Gmail; withUpdates: boolean; updateFeed: { stable?: Release; canary?: Release }; updateFirstCheckSecs: number; updates: Updates; withSentry: boolean; sentry: FakeSentry; withTelemetry: boolean; telemetry: FakeOtlp; builtinApps: boolean; feedbackEmail: string | null; appEnv: Record<string, string>; phone: boolean }>({
  /** A phone-sized window (`test.use({ phone: true })`): the compact layout (docs/ANDROID.md). */
  phone: [false, { option: true }],
  /** Act as a release build with PeetMail's own apps built in (the fakes' client IDs). */
  builtinApps: [false, { option: true }],
  /** The address Help → Report a problem offers to email (`PEETMAIL_FEEDBACK_EMAIL`). */
  feedbackEmail: [null, { option: true }],
  /** Any other test knobs for the app (`PEETMAIL_*`, see testenv.rs). */
  appEnv: [{}, { option: true }],
  /** Send usage telemetry to a fake OTLP collector (`test.use({ withTelemetry: true })`), as `telemetry`. */
  withTelemetry: [false, { option: true }],
  telemetry: async ({ withTelemetry }, use) => {
    if (!withTelemetry) return use(null as unknown as FakeOtlp);
    const fake = await startFakeOtlp();
    await use(fake);
    await fake.close();
  },
  /** Send crash reports to a fake Sentry (`test.use({ withSentry: true })`), as `sentry`. */
  withSentry: [false, { option: true }],
  sentry: async ({ withSentry }, use) => {
    if (!withSentry) return use(null as unknown as FakeSentry);
    const fake = await startFakeSentry();
    await use(fake);
    await fake.close();
  },
  /** Serve update feeds (`test.use({ withUpdates: true })`), as `updates`. */
  withUpdates: [false, { option: true }],
  /** Releases on the feeds before the app starts (the background check can run before the test body). */
  updateFeed: [{}, { option: true }],
  /** Seconds after start before the app first checks for updates on its own. */
  updateFirstCheckSecs: [3600, { option: true }],
  updates: async ({ withUpdates, updateFeed }, use) => {
    if (!withUpdates) return use(null as unknown as Updates);
    const fake = await startFakeUpdates();
    if (updateFeed.stable) fake.publish("stable", updateFeed.stable);
    if (updateFeed.canary) fake.publish("canary", updateFeed.canary);
    const installedPath = join(tmpdir(), `peetmail-update-installed-${process.pid}-${Date.now()}-${Math.random().toString(36).slice(2)}.json`);
    await use(Object.assign(fake, { installedPath }));
    await fake.close();
    await rm(installedPath, { force: true });
  },
  /** Serve a fake Gmail too (`test.use({ withGmail: true })`), as `gmail`. */
  withGmail: [false, { option: true }],
  /** Extra filler messages in the fake Gmail inbox. */
  gmailExtra: [0, { option: true }],
  gmail: async ({ withGmail, gmailExtra }, use) => {
    if (!withGmail) return use(null as unknown as Gmail);
    const gmail = await startFakeGmail({ extra: gmailExtra });
    await use(gmail);
    await gmail.close();
  },
  /** Serve a second mailbox too (`test.use({ secondAccount: true })`), then use `pair`. */
  secondAccount: [false, { option: true }],
  /** The two-mailbox fake (only with secondAccount): `pair.second`, `pair.signInAs("b")`. */
  pair: async ({ fake, secondAccount }, use) => {
    if (!secondAccount) throw new Error("the pair fixture needs test.use({ secondAccount: true })");
    await use(fake as Pair);
  },
  /** Undo-send delay for this test's app (`test.use({ undoSendSecs: 3 })`); 0 sends at once. */
  undoSendSecs: [0, { option: true }],

  // eslint-disable-next-line no-empty-pattern
  fake: async ({ secondAccount }, use) => {
    const fake = secondAccount ? await startFakePair() : await startFakeGraph();
    await use(fake);
    await fake.close();
  },

  app: async ({ fake, undoSendSecs, gmail, updates, updateFirstCheckSecs, sentry, telemetry, builtinApps, feedbackEmail, appEnv, phone }, use, testInfo) => {
    const extraEnv: Record<string, string> = {
      ...(gmail ? { PEETMAIL_GMAIL_URL: gmail.gmailUrl, PEETMAIL_GOOGLE_AUTH_URL: gmail.authUrl, PEETMAIL_GCAL_URL: `${gmail.origin}/calendar/v3` } : {}),
      ...(updates
        ? {
            PEETMAIL_UPDATE_URL: updates.url,
            PEETMAIL_UPDATE_FIRST_CHECK_SECS: String(updateFirstCheckSecs),
            PEETMAIL_FAKE_UPDATE_INSTALL: updates.installedPath,
          }
        : {}),
      ...(sentry ? { PEETMAIL_SENTRY_DSN: sentry.dsn } : {}),
      ...(feedbackEmail ? { PEETMAIL_FEEDBACK_EMAIL: feedbackEmail } : {}),
      ...appEnv,
      ...(builtinApps
        ? {
            PEETMAIL_BUILTIN_MS_CLIENT_ID: CLIENT_ID,
            PEETMAIL_BUILTIN_GOOGLE_CLIENT_ID: GOOGLE_CLIENT_ID,
            PEETMAIL_BUILTIN_GOOGLE_CLIENT_SECRET: GOOGLE_CLIENT_SECRET,
          }
        : {}),
      ...(telemetry
        ? {
            PEETMAIL_OTLP_ENDPOINT: telemetry.url,
            PEETMAIL_OTLP_HEADERS: "authorization=Bearer test-ingest-token",
            PEETMAIL_OTLP_METRIC_INTERVAL_MS: "500",
            // The SDK's own knobs: export batches quickly so tests don't wait 5s.
            OTEL_BSP_SCHEDULE_DELAY: "200",
            OTEL_BLRP_SCHEDULE_DELAY: "200",
          }
        : {}),
    };
    const app = await launchApp({ fake, undoSendSecs, extraEnv, viewport: phone ? PHONE_VIEWPORT : undefined });
    const consoleLines: string[] = [];
    app.page.on("console", (m) => consoleLines.push(`[${m.type()}] ${m.text()}`));
    app.page.on("pageerror", (e) => consoleLines.push(`[pageerror] ${e.stack ?? e.message}`));
    await use(app);
    if (consoleLines.length) await testInfo.attach("console.log", { body: consoleLines.join("\n"), contentType: "text/plain" });
    const shot = await app.page.screenshot().catch(() => null);
    if (shot) await testInfo.attach("final-screen.png", { body: shot, contentType: "image/png" });
    const log = await readFile(app.logPath, "utf8").catch(() => "");
    if (log) await testInfo.attach("app.log", { body: log, contentType: "text/plain" });
    await app.close();
  },

  page: async ({ app }, use) => {
    await use(app.page);
  },

  inbox: async ({ app, fake }, use) => {
    await signIn(app.page, fake.clientId);
    await waitForSyncIdle(app.page);
    // Background body prefetch runs after the first sync; let it finish so tests that
    // count requests see only their own traffic.
    await waitForQuiet(fake);
    await use(app.page);
  },
});

export { expect };

/** Default cache window in the app (state.rs DEFAULT_CACHE_DAYS). */
export const CACHE_DAYS = 90;

/** Conversations in the inbox matching `pred`, inside the cache window (mirrors the app's SQL). */
export function inboxConversations(pred: (m: ReturnType<typeof defaultMailbox>[number]) => boolean = () => true) {
  const cutoff = new Date(Date.now() - CACHE_DAYS * 86_400_000).toISOString();
  return new Set(
    defaultMailbox()
      .filter((m) => m.parentFolderId === FOLDER_IDS.inbox && m.receivedDateTime >= cutoff && pred(m))
      .map((m) => m.conversationId),
  ).size;
}

/** What the default view (the built-in Inbox tab: everything the default Other split doesn't claim) shows after a fresh sync. */
export const DEFAULT_CONVERSATIONS = inboxConversations((m) => m.inferenceClassification === "focused");

/** Opens Settings at a page ("accounts", "mail", "calendar", "notifications", "storage", "updates", "privacy", "apps"). */
export async function openSettings(page: Page, section: string) {
  await page.keyboard.press("Control+,");
  await showSettingsPage(page, section);
}

/** Switches the open Settings dialog to a page. */
export async function showSettingsPage(page: Page, section: string) {
  await page.getByTestId(`settings-nav-${section}`).click();
  await expect(page.getByTestId("settings")).toHaveAttribute("data-page", section);
}

/**
 * Conversation ids cached in a folder (by role: inbox, archive, deleteditems...), across
 * accounts or for the one with `email`. Straight from the app's cache, for folders that
 * have no view yet.
 */
export async function folderConversations(page: Page, role: string, email?: string): Promise<string[]> {
  return page.evaluate(async ({ role, email }) => {
    const invoke = (window as any).__TAURI_INTERNALS__.invoke;
    const boot = await invoke("bootstrap");
    const out: string[] = [];
    for (const a of boot.accounts.filter((a: any) => !email || a.email === email)) {
      const f = (await invoke("list_folders", { accountId: a.id })).find((f: any) => f.role === role);
      if (f) out.push(...(await invoke("list_threads", { folderId: f.id, limit: 1000 })).map((t: any) => t.conversationId));
    }
    return out.sort();
  }, { role, email });
}

export async function waitForSyncIdle(page: Page) {
  await expect(page.getByTestId("sync-status")).toHaveAttribute("data-state", "idle", { timeout: 15_000 });
}

/** Waits until the fake has received no requests for `quietMs`. */
export async function waitForQuiet(fake: Fake, quietMs = 400, timeoutMs = 15_000) {
  const deadline = Date.now() + timeoutMs;
  let last = fake.state.requests.length;
  let since = Date.now();
  while (Date.now() < deadline) {
    await new Promise((r) => setTimeout(r, 50));
    const n = fake.state.requests.length;
    if (n !== last) {
      last = n;
      since = Date.now();
    } else if (Date.now() - since >= quietMs) {
      return;
    }
  }
  throw new Error(`fake server still busy after ${timeoutMs}ms`);
}

/** GETs of one message's body. */
export function bodyRequests(fake: Fake, messageId: string, since = 0) {
  return fake.requestsSince(since).filter((r) => r.method === "GET" && r.path === `/v1.0/me/messages/${messageId}`);
}

/** Presses Ctrl+R and waits until the app has made a delta request and gone idle again. */
export async function syncNow(page: Page, fake: Fake) {
  const before = fake.state.requests.length;
  await page.keyboard.press("Control+r");
  await expect.poll(() => fake.requestsSince(before).some((r) => r.path.endsWith("/delta") && r.status === 200)).toBe(true);
  await waitForSyncIdle(page);
}

/** Makes sure the right pane shows the calendar (the default; `0` toggles it with Recent Opens). */
export async function showCalendarPane(page: Page) {
  await expect(page.getByTestId("right-pane")).toBeVisible();
  if ((await page.getByTestId("pane-tab-calendar").getAttribute("aria-selected")) !== "true") await page.keyboard.press("0");
  await expect(page.getByTestId("day-view")).toBeVisible();
}

/** Switches the right pane to Recent Opens. */
export async function showRecentPane(page: Page) {
  await page.getByTestId("pane-tab-recent").click();
  await expect(page.getByTestId("pane-tab-recent")).toHaveAttribute("aria-selected", "true");
}

export function row(page: Page, conversationId: string) {
  return page.locator(`[data-testid=thread-row][data-conversation-id="${conversationId}"]`);
}

export function selectedRow(page: Page) {
  return page.locator("[data-testid=thread-row][aria-selected=true]");
}

/**
 * Jumps to the top (g g), then moves down with j until `conversationId` is selected, without opening it
 * (a click opens the conversation, which also marks it read).
 */
export async function selectWithKeys(page: Page, conversationId: string) {
  await page.keyboard.press("g");
  await page.keyboard.press("g");
  for (let i = 0; i < 300; i++) {
    if ((await selectedRow(page).getAttribute("data-conversation-id")) === conversationId) return;
    await page.keyboard.press("j");
  }
  throw new Error(`couldn't reach ${conversationId} with j`);
}

export function messageBody(page: Page, nth = 0) {
  return page.frameLocator("[data-testid=message-body]").nth(nth);
}

/** Attaches a named screenshot to the report mid-test. */
export async function snap(page: Page, testInfo: TestInfo, name: string) {
  await testInfo.attach(`${name}.png`, { body: await page.screenshot(), contentType: "image/png" });
}
