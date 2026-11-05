// Settings sync (docs/SYNC.md, docs/user/sync.md): two PeetMail instances, each with its own
// data dir, signed in to the same fake Graph mailbox, syncing through the real sync server
// (server/, built by global-setup). The server trusts the fake Graph's id_tokens.

import type { Page } from "@playwright/test";
import { test, expect, openSettings, waitForSyncIdle } from "./fixtures";
import { launchApp, signIn, type LaunchedApp } from "../harness/app.ts";
import { WORK_ACCOUNT } from "../harness/fake-pair.ts";
import { startSyncServer, type SyncServer } from "../harness/sync-server.ts";

const PASSPHRASE = "correct horse battery staple";

// A server per test, on the test's fake Graph.
const it = test.extend<{ server: SyncServer }>({
  server: async ({ fake }, use) => {
    const server = await startSyncServer({ authorityUrl: fake.authorityUrl });
    await use(server);
    await server.close();
  },
});

const invoke = <T = any>(page: Page, cmd: string, args?: Record<string, unknown>): Promise<T> =>
  page.evaluate(([c, a]) => (window as any).__TAURI_INTERNALS__.invoke(c, a), [cmd, args] as const);

const syncSettings = (page: Page) => invoke(page, "settings_sync_now");
const snippets = async (page: Page) => (await invoke<{ name: string; body: string }[]>(page, "list_snippets")).map((s) => `${s.name}: ${s.body}`);

/** Turns sync on for the first device: a new sync account. Returns the recovery key. */
async function turnOnFirst(page: Page, server: SyncServer): Promise<string> {
  await openSettings(page, "sync");
  await page.getByTestId("sync-server").fill(server.url);
  await page.getByTestId("sync-connect").click();
  await page.getByTestId("sync-passphrase").fill(PASSPHRASE);
  await page.getByTestId("sync-passphrase-confirm").fill(PASSPHRASE);
  await page.getByTestId("sync-create").click();
  const key = (await page.getByTestId("sync-recovery-key").textContent())?.trim() ?? "";
  expect(key).toMatch(/^[A-Z2-7]{4}(-[A-Z2-7]{1,4})+$/);
  await page.getByTestId("sync-recovery-done").click();
  await expect(page.getByTestId("sync-section")).toHaveAttribute("data-state", "idle", { timeout: 15_000 });
  return key;
}

/** Connects a device whose account already has a sync account: it asks for the passphrase. */
async function connectExisting(page: Page, server: SyncServer) {
  await openSettings(page, "sync");
  await page.getByTestId("sync-server").fill(server.url);
  await page.getByTestId("sync-connect").click();
  await expect(page.getByTestId("sync-secret")).toBeVisible();
}

async function unlock(page: Page, secret: string) {
  await page.getByTestId("sync-secret").fill(secret);
  await page.getByTestId("sync-join").click();
  await expect(page.getByTestId("sync-section")).toHaveAttribute("data-state", "idle", { timeout: 15_000 });
}

/** A second PC: its own data dir, signed in to the same mailbox. */
async function secondDevice(fake: Parameters<typeof launchApp>[0]["fake"] & { clientId: string }): Promise<LaunchedApp> {
  const b = await launchApp({ fake });
  await signIn(b.page, fake.clientId);
  await waitForSyncIdle(b.page);
  return b;
}

it("settings made on one PC appear on another, the later edit wins, and the server can't read any of it", async ({ inbox: a, fake, server }) => {
  // Settings made before sync was ever on go up when it's turned on.
  await invoke(a, "set_snippets", { snippets: [{ name: "thanks", body: "Thanks so much, {first_name}! Zanzibar-canary" }] });
  await invoke(a, "set_signatures", { signatures: [{ address: "user@contoso.com", html: "<p>Priya Nair · Quokka-canary</p>", replies: false, managed: false }] });
  await turnOnFirst(a, server);
  await a.keyboard.press("Escape");

  const b = await secondDevice(fake);
  try {
    await connectExisting(b.page, server);
    await unlock(b.page, PASSPHRASE);
    await expect.poll(() => snippets(b.page), { timeout: 15_000 }).toEqual(["thanks: Thanks so much, {first_name}! Zanzibar-canary"]);
    // B's Settings shows what it has linked and the two devices.
    await syncSettings(b.page);
    await b.page.keyboard.press("Escape");
    await openSettings(b.page, "sync");
    await expect(b.page.getByTestId("sync-device")).toHaveCount(2);
    await expect(b.page.locator("[data-testid=sync-device][data-current=true]")).toHaveCount(1);
    await expect(b.page.getByTestId("sync-identity")).toHaveCount(1);
    await b.page.keyboard.press("Escape");

    // A split made on B shows up as a tab on A, without A doing anything but syncing.
    await invoke(b.page, "set_splits", { splits: [{ name: "Other", query: "is:other", id: "other" }, { name: "Mole-canary", query: "from:boss@contoso.com" }] });
    await syncSettings(b.page);
    await syncSettings(a);
    await expect(a.getByTestId("split-tab").filter({ hasText: "Mole-canary" })).toBeVisible({ timeout: 10_000 });

    // Both edit the same snippet: A first, B later. Whichever syncs first, B's edit wins on both.
    await invoke(a, "set_snippets", { snippets: [{ name: "thanks", body: "From A" }] });
    await new Promise((r) => setTimeout(r, 20));
    await invoke(b.page, "set_snippets", { snippets: [{ name: "thanks", body: "From B" }] });
    await syncSettings(b.page);
    await syncSettings(a);
    await syncSettings(b.page);
    expect(await snippets(a)).toEqual(["thanks: From B"]);
    expect(await snippets(b.page)).toEqual(["thanks: From B"]);

    // Device-only settings stay put.
    await invoke(a, "set_cache_limits", { limits: { cacheDays: 30, bodyCacheMb: 500 } });
    await syncSettings(a);
    await syncSettings(b.page);
    expect((await invoke(b.page, "bootstrap")).cacheLimits.cacheDays).toBe(90);
  } finally {
    await b.close();
  }

  // The canary: none of what was synced is readable on the server.
  const stored = server.databaseBytes().toString("latin1") + server.log();
  for (const secret of ["Zanzibar-canary", "Quokka-canary", "Mole-canary", "boss@contoso.com", "snippet:", "signature:", "split:"]) {
    expect(stored, `${secret} reached the server in the clear`).not.toContain(secret);
  }
});

it("a wrong passphrase is refused; the recovery key unlocks instead", async ({ inbox: a, fake, server }) => {
  const recovery = await turnOnFirst(a, server);
  const b = await secondDevice(fake);
  try {
    await connectExisting(b.page, server);
    await b.page.getByTestId("sync-secret").fill("not the passphrase at all");
    await b.page.getByTestId("sync-join").click();
    await expect(b.page.getByTestId("sync-error")).toContainText("isn't right");
    await unlock(b.page, recovery.toLowerCase().replaceAll("-", " "));
    // A shows the same key from its own Settings.
    await a.getByTestId("sync-show-recovery").click();
    await expect(a.getByTestId("sync-shown-recovery-key")).toHaveText(recovery);
  } finally {
    await b.close();
  }
});

it("a device signed out from another PC stops syncing and says so", async ({ inbox: a, fake, server }) => {
  await turnOnFirst(a, server);
  const b = await secondDevice(fake);
  try {
    await connectExisting(b.page, server);
    await unlock(b.page, PASSPHRASE);
    await a.getByTestId("sync-now").click();
    await expect(a.getByTestId("sync-device")).toHaveCount(2);
    await a.locator("[data-testid=sync-device][data-current=false]").getByTestId("sync-device-revoke").click();
    await expect(a.getByTestId("sync-device")).toHaveCount(1);

    await invoke(b.page, "set_snippets", { snippets: [{ name: "late", body: "Never arrives" }] });
    await syncSettings(b.page);
    await expect(b.page.getByTestId("sync-section")).toHaveAttribute("data-state", "off");
    await expect(b.page.getByTestId("sync-off-reason")).toContainText("signed out");
    await syncSettings(a);
    expect(await snippets(a)).toEqual([]);
  } finally {
    await b.close();
  }
});

it("turning sync off keeps this PC's settings, and turning it on again rejoins", async ({ inbox: a, server }) => {
  await invoke(a, "set_snippets", { snippets: [{ name: "kept", body: "Still here" }] });
  await turnOnFirst(a, server);
  await a.getByTestId("sync-disable").click();
  await expect(a.getByTestId("sync-section")).toHaveAttribute("data-state", "off");
  expect(await snippets(a)).toEqual(["kept: Still here"]);
  // The account still has its sync account, so it's the passphrase again, not a new one.
  await a.getByTestId("sync-server").fill(server.url);
  await a.getByTestId("sync-connect").click();
  await unlock(a, PASSPHRASE);
  await expect(a.getByTestId("sync-device")).toHaveCount(1);
});

it("plain http is only for this PC, and an address that isn't a sync server says so", async ({ inbox: a, fake }) => {
  await openSettings(a, "sync");
  await a.getByTestId("sync-server").fill("http://sync.example.com");
  await a.getByTestId("sync-connect").click();
  await expect(a.getByTestId("sync-error")).toContainText("https://");
  // The fake Graph answers HTTP, but not like a sync server.
  await a.getByTestId("sync-server").fill(fake.authorityUrl.replace(/\/oauth$/, ""));
  await a.getByTestId("sync-connect").click();
  await expect(a.getByTestId("sync-error")).toContainText("isn't a PeetMail sync server");
});

it.describe("with two accounts", () => {
  it.use({ secondAccount: true });

  type Pair = Parameters<Parameters<typeof it>[2]>[0]["pair"];
  const accountRow = (page: Page, email: string) => page.locator(`[data-testid=account-row][data-email="${email}"]`);
  const offered = (page: Page, email: string) => page.locator(`[data-testid=synced-account][data-email="${email}"]`);

  /** Adds the work mailbox on a PC (from Settings → Accounts, which is left open). */
  async function addWork(page: Page, pair: Pair) {
    pair.signInAs("b");
    await openSettings(page, "accounts");
    await page.getByTestId("account-add").click();
    await expect(accountRow(page, WORK_ACCOUNT.mail)).toBeVisible({ timeout: 15_000 });
    pair.signInAs("a");
  }

  /** A with sync on and the work mailbox; B signed in to the default mailbox only, joined. */
  async function twoPcs(a: Page, pair: Pair, server: SyncServer) {
    await turnOnFirst(a, server);
    await addWork(a, pair);
    await syncSettings(a);
    await a.keyboard.press("Escape");
    const b = await secondDevice(pair);
    await connectExisting(b.page, server);
    await unlock(b.page, PASSPHRASE);
    return b;
  }

  async function signOutWork(page: Page, choice: "here" | "everywhere") {
    await openSettings(page, "accounts");
    await accountRow(page, WORK_ACCOUNT.mail).getByTestId("account-signout").click();
    await accountRow(page, WORK_ACCOUNT.mail).getByTestId(`account-signout-${choice}`).click();
    await expect(accountRow(page, WORK_ACCOUNT.mail)).toHaveCount(0);
  }

  it("an account added on one PC is offered on the other, and signing in there adds it", async ({ inbox: a, pair, server }) => {
    const b = await twoPcs(a, pair, server);
    try {
      // B hears about it as it arrives, once.
      await expect(b.page.getByTestId("toast")).toContainText(`${WORK_ACCOUNT.mail} is on your other PC`, { timeout: 15_000 });
      await b.page.keyboard.press("Escape");
      await openSettings(b.page, "accounts");
      await expect(offered(b.page, WORK_ACCOUNT.mail)).toContainText("not signed in on this PC");
      // The account B already has isn't offered.
      await expect(b.page.getByTestId("synced-account")).toHaveCount(1);
      pair.signInAs("b");
      await offered(b.page, WORK_ACCOUNT.mail).getByTestId("synced-account-signin").click();
      await expect(accountRow(b.page, WORK_ACCOUNT.mail)).toBeVisible({ timeout: 15_000 });
      await expect(b.page.getByTestId("synced-account")).toHaveCount(0);
      // The sign-in page was told which account to suggest.
      expect(pair.second.state.authorizations.at(-1)?.login_hint).toBe(WORK_ACCOUNT.mail);
    } finally {
      await b.close();
    }
  });

  it("signing out on this PC only leaves the other PCs alone; everywhere signs them out too", async ({ inbox: a, pair, server }) => {
    const b = await twoPcs(a, pair, server);
    try {
      await b.page.keyboard.press("Escape");
      await openSettings(b.page, "accounts");
      pair.signInAs("b");
      await offered(b.page, WORK_ACCOUNT.mail).getByTestId("synced-account-signin").click();
      await expect(accountRow(b.page, WORK_ACCOUNT.mail)).toBeVisible({ timeout: 15_000 });
      pair.signInAs("a");

      // This PC only: gone from B, not offered there again, still on A.
      await signOutWork(b.page, "here");
      await expect(offered(b.page, WORK_ACCOUNT.mail)).toHaveCount(0);
      await syncSettings(b.page);
      await syncSettings(a);
      await openSettings(a, "accounts");
      await expect(accountRow(a, WORK_ACCOUNT.mail)).toBeVisible();
      await a.keyboard.press("Escape");

      // Back on B, then signed out everywhere from B: A signs it out too.
      await b.page.keyboard.press("Escape");
      await addWork(b.page, pair);
      await signOutWork(b.page, "everywhere");
      await syncSettings(b.page);
      await syncSettings(a);
      await openSettings(a, "accounts");
      await expect(accountRow(a, WORK_ACCOUNT.mail)).toHaveCount(0, { timeout: 10_000 });
      await expect(a.getByTestId("account-row")).toHaveCount(1); // its own account stays
      await expect(offered(a, WORK_ACCOUNT.mail)).toHaveCount(0);
    } finally {
      await b.close();
    }
  });

  it("the other accounts on a PC are linked automatically, so either signs a new PC in", async ({ inbox: a, pair, server }) => {
    pair.signInAs("b");
    await openSettings(a, "accounts");
    await a.getByTestId("account-add").click();
    await expect(a.locator(`[data-testid=account-row][data-email="${WORK_ACCOUNT.mail}"]`)).toBeVisible({ timeout: 15_000 });
    await a.keyboard.press("Escape");
    await waitForSyncIdle(a);
    await turnOnFirst(a, server);
    await expect.poll(async () => (await invoke(a, "settings_sync_identities")).length, { timeout: 15_000 }).toBe(2);
    await expect.poll(async () => (await invoke(a, "settings_sync_status")).accounts.every((x: { state: string }) => x.state === "linked")).toBe(true);
  });
});
