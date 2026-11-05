import { fileURLToPath } from "node:url";
import { beforeEach, describe, expect, test, vi } from "vitest";
import { LIMITS, type PushItem, type Session } from "#lib/protocol.js";
import type { Config } from "./config.js";
import { openDb } from "./db.js";
import { SyncError } from "./errors.js";
import type { Identity, Verifier } from "./oidc.js";
import { createService, mayRegister, type Auth, type Service } from "./service.js";

const MIGRATIONS = fileURLToPath(new URL("../../../drizzle", import.meta.url));

/** Tokens are just the identity, as JSON: these tests are about the service, not the verifier (oidc.test.ts). */
const token = (subject: string, email: string | null = `${subject}@example.com`, tenant: string | null = null) =>
  JSON.stringify({ provider: "microsoft", subject, email, tenant } satisfies Identity);
const verify: Verifier = async (_provider, idToken) => {
  if (idToken === "bad") throw new SyncError(401, "invalid_token", "refused");
  return JSON.parse(idToken);
};

let clock: number;
let svc: Service;
const config: Config = {
  databasePath: ":memory:",
  migrationsDir: MIGRATIONS,
  allowRegister: [{ kind: "domain", value: "example.com" }],
  clientIds: { microsoft: ["app"], google: [] },
  discoveryUrls: { microsoft: "", google: "" },
};

beforeEach(() => {
  clock = Date.parse("2026-11-05T00:00:00Z");
  svc = createService(openDb(":memory:", MIGRATIONS), verify, config, () => clock);
  vi.spyOn(console, "log").mockImplementation(() => {});
});

const key = { wrappedKey: "d3JhcHBlZA", kdfParams: { alg: "argon2id", m: 65536, t: 3, p: 1, salt: "c2FsdA" } };
const register = (subject: string, deviceName = "Desk") => svc.register({ provider: "microsoft", idToken: token(subject), deviceName, ...key });
const authOf = (s: Session): Auth => svc.authenticate(`Bearer ${s.deviceToken}`);
const change = (id: string, blob: string, baseSeq: number | null = null, deleted = false): PushItem => ({ id, blob, baseSeq, deleted });
const fails = (fn: () => unknown, status: number, code: string) => expect(fn).toThrow(expect.objectContaining({ status, code }));
const failsAsync = (p: Promise<unknown>, status: number, code: string) => expect(p).rejects.toMatchObject({ status, code });

describe("accounts and devices", () => {
  test("registering creates a sync account and signs the device in", async () => {
    const s = await register("priya");
    expect(s.deviceToken).toMatch(/^pmd_[A-Za-z0-9_-]{43}$/);
    expect(s).toMatchObject(key);
    expect(authOf(s).deviceId).toBe(s.deviceId);
  });

  test("another device signs in with the same mail account and gets the wrapped key", async () => {
    const first = await register("priya");
    const second = await svc.signIn({ provider: "microsoft", idToken: token("priya"), deviceName: "Laptop" });
    expect(second.deviceId).not.toBe(first.deviceId);
    expect(second).toMatchObject(key);
    expect(authOf(second).userId).toBe(authOf(first).userId);
    expect(svc.listDevices(authOf(second)).map((d) => [d.name, d.current])).toEqual([["Desk", false], ["Laptop", true]]);
  });

  test("an unlinked mail account isn't silently given a new sync account", async () => {
    await failsAsync(svc.signIn({ provider: "microsoft", idToken: token("nobody"), deviceName: "Desk" }), 404, "no_user");
    expect(svc.userCount()).toBe(0);
  });

  test("registering twice with one mail account is refused", async () => {
    await register("priya");
    await failsAsync(register("priya"), 409, "already_registered");
  });

  test("only allowed accounts may register", async () => {
    await failsAsync(svc.register({ provider: "microsoft", idToken: token("eve", "eve@elsewhere.net"), deviceName: "x", ...key }), 403, "registration_closed");
    await failsAsync(svc.register({ provider: "microsoft", idToken: token("anon", null), deviceName: "x", ...key }), 403, "registration_closed");
    await failsAsync(svc.register({ provider: "microsoft", idToken: "bad", deviceName: "x", ...key }), 401, "invalid_token");
  });

  test("tokens: missing, unknown and revoked ones are refused", async () => {
    const desk = await register("priya");
    const laptop = await svc.signIn({ provider: "microsoft", idToken: token("priya"), deviceName: "Laptop" });
    fails(() => svc.authenticate(null), 401, "unauthorized");
    fails(() => svc.authenticate("Bearer pmd_nope"), 401, "unauthorized");
    fails(() => svc.authenticate(desk.deviceToken), 401, "unauthorized"); // no "Bearer"
    svc.revokeDevice(authOf(desk), laptop.deviceId);
    fails(() => authOf(laptop), 401, "device_revoked");
    expect(svc.listDevices(authOf(desk)).map((d) => d.name)).toEqual(["Desk"]);
    // Revoking your own device signs it out.
    svc.revokeDevice(authOf(desk), desk.deviceId);
    fails(() => authOf(desk), 401, "device_revoked");
  });

  test("a device can't revoke another user's device", async () => {
    const priya = await register("priya");
    const sam = await register("sam");
    fails(() => svc.revokeDevice(authOf(priya), sam.deviceId), 404, "not_found");
    expect(authOf(sam).deviceId).toBe(sam.deviceId);
  });

  test("last seen is updated, but not on every request", async () => {
    const s = await register("priya");
    clock += 60_000;
    authOf(s);
    expect(svc.listDevices(authOf(s))[0].lastSeenAt).toBe("2026-11-05T00:00:00.000Z");
    clock += 10 * 60_000;
    authOf(s);
    expect(svc.listDevices(authOf(s))[0].lastSeenAt).toBe("2026-11-05T00:11:00.000Z");
  });

  test("a new passphrase rewraps the key for the next device", async () => {
    const s = await register("priya");
    svc.setKey(authOf(s), { wrappedKey: "bmV3", kdfParams: { alg: "argon2id", salt: "bmV3" } });
    const next = await svc.signIn({ provider: "microsoft", idToken: token("priya"), deviceName: "Laptop" });
    expect(next.wrappedKey).toBe("bmV3");
  });

  test("deleting the sync account takes everything with it", async () => {
    const s = await register("priya");
    svc.push(authOf(s), [change("a", "QQ")]);
    svc.deleteUser(authOf(s));
    fails(() => authOf(s), 401, "unauthorized");
    await failsAsync(svc.signIn({ provider: "microsoft", idToken: token("priya"), deviceName: "x" }), 404, "no_user");
    // The mail account is free to start again.
    const again = await register("priya");
    expect(svc.pull(authOf(again), 0, 100).items).toEqual([]);
  });
});

describe("several mail accounts", () => {
  test("a linked account signs in to the same sync account", async () => {
    const s = await register("work");
    const linked = await svc.linkIdentity(authOf(s), { provider: "microsoft", idToken: token("personal", "me@outlook.com") });
    expect(linked).toMatchObject({ provider: "microsoft", email: "me@outlook.com" });
    const other = await svc.signIn({ provider: "microsoft", idToken: token("personal", "me@outlook.com"), deviceName: "Home" });
    expect(authOf(other).userId).toBe(authOf(s).userId);
    // Linking doesn't need ALLOW_REGISTER: outlook.com isn't allowed to register here.
    expect(svc.listIdentities(authOf(s)).map((i) => i.email)).toEqual(["work@example.com", "me@outlook.com"]);
  });

  test("linking again is harmless; linking another user's account is refused", async () => {
    const priya = await register("priya");
    const sam = await register("sam");
    const first = await svc.linkIdentity(authOf(priya), { provider: "microsoft", idToken: token("priya") });
    expect(first.email).toBe("priya@example.com");
    expect(svc.listIdentities(authOf(priya))).toHaveLength(1);
    await failsAsync(svc.linkIdentity(authOf(priya), { provider: "microsoft", idToken: token("sam") }), 409, "linked_elsewhere");
    expect(svc.listIdentities(authOf(sam))).toHaveLength(1);
  });

  test("unlinking keeps at least one account, and only touches your own", async () => {
    const s = await register("work");
    const personal = await svc.linkIdentity(authOf(s), { provider: "microsoft", idToken: token("personal") });
    const [work] = svc.listIdentities(authOf(s));
    const sam = await register("sam");
    fails(() => svc.unlinkIdentity(authOf(sam), personal.id), 404, "not_found");
    svc.unlinkIdentity(authOf(s), personal.id);
    fails(() => svc.unlinkIdentity(authOf(s), work.id), 409, "last_identity");
    await failsAsync(svc.signIn({ provider: "microsoft", idToken: token("personal"), deviceName: "x" }), 404, "no_user");
  });
});

describe("items", () => {
  test("new items are stored under increasing versions and pulled in order", async () => {
    const s = authOf(await register("priya"));
    expect(svc.push(s, [change("a", "QQ"), change("b", "Qg")]).results).toEqual([
      { id: "a", status: "ok", seq: 1 },
      { id: "b", status: "ok", seq: 2 },
    ]);
    expect(svc.pull(s, 0, 100)).toEqual({
      items: [
        { id: "a", seq: 1, blob: "QQ", deleted: false },
        { id: "b", seq: 2, blob: "Qg", deleted: false },
      ],
      cursor: 2,
      more: false,
    });
    expect(svc.pull(s, 2, 100)).toEqual({ items: [], cursor: 2, more: false });
  });

  test("a change on the current version wins; one on an old version gets the server's copy back", async () => {
    const desk = authOf(await register("priya"));
    const laptop = authOf(await svc.signIn({ provider: "microsoft", idToken: token("priya"), deviceName: "Laptop" }));
    svc.push(desk, [change("a", "djE")]); // seq 1
    expect(svc.push(laptop, [change("a", "djI", 1)]).results).toEqual([{ id: "a", status: "ok", seq: 2 }]);
    // The desk still thinks it's on 1.
    expect(svc.push(desk, [change("a", "djM", 1)]).results).toEqual([{ id: "a", status: "conflict", seq: 2, blob: "djI", deleted: false }]);
    // A device that has never seen the item but makes one with the same id also conflicts.
    expect(svc.push(desk, [change("a", "djQ", null)]).results[0].status).toBe("conflict");
    // Having merged, it pushes on 2.
    expect(svc.push(desk, [change("a", "djU", 2)]).results).toEqual([{ id: "a", status: "ok", seq: 3 }]);
  });

  test("a conflict on one item doesn't stop the others", async () => {
    const s = authOf(await register("priya"));
    svc.push(s, [change("a", "QQ")]);
    expect(svc.push(s, [change("a", "QQ", 0), change("b", "Qg")]).results.map((r) => r.status)).toEqual(["conflict", "ok"]);
  });

  test("deletions are tombstones that pull like any change", async () => {
    const s = authOf(await register("priya"));
    svc.push(s, [change("a", "QQ")]);
    svc.push(s, [change("a", "dG9tYg", 1, true)]);
    expect(svc.pull(s, 1, 100).items).toEqual([{ id: "a", seq: 2, blob: "dG9tYg", deleted: true }]);
  });

  test("pulls page by version", async () => {
    const s = authOf(await register("priya"));
    svc.push(s, ["a", "b", "c", "d", "e"].map((id) => change(id, "QQ")));
    const one = svc.pull(s, 0, 2);
    expect([one.items.map((i) => i.id), one.cursor, one.more]).toEqual([["a", "b"], 2, true]);
    const two = svc.pull(s, one.cursor, 2);
    const three = svc.pull(s, two.cursor, 2);
    expect([three.items.map((i) => i.id), three.more]).toEqual([["e"], false]);
  });

  test("users can't see each other's items, and versions are per user", async () => {
    const priya = authOf(await register("priya"));
    const sam = authOf(await register("sam"));
    svc.push(priya, [change("a", "cHJpeWE")]);
    expect(svc.push(sam, [change("a", "c2Ft")]).results).toEqual([{ id: "a", status: "ok", seq: 1 }]);
    expect(svc.pull(sam, 0, 100).items.map((i) => i.blob)).toEqual(["c2Ft"]);
    expect(svc.pull(priya, 0, 100).items.map((i) => i.blob)).toEqual(["cHJpeWE"]);
  });

  test("a sync account has a cap, and a push over it changes nothing", async () => {
    const s = authOf(await register("priya"));
    const many = Array.from({ length: LIMITS.pushItems }, (_, i) => change(`i${i}`, "QQ"));
    for (let n = 0; n < LIMITS.userItems / LIMITS.pushItems; n++) {
      svc.push(s, many.map((c) => ({ ...c, id: `${n}-${c.id}` })));
    }
    fails(() => svc.push(s, [change("one-more", "QQ")]), 413, "too_many_items");
    // Updating existing ones is still fine.
    expect(svc.push(s, [change("0-i0", "Qg", 1)]).results[0].status).toBe("ok");
    expect(svc.pull(s, LIMITS.userItems, 10).items.map((i) => i.id)).toEqual(["0-i0"]);
  });
});

test("ALLOW_REGISTER rules", () => {
  const ms = (email: string | null, tenant: string | null = null): Identity => ({ provider: "microsoft", subject: "s", email, tenant });
  const google = (email: string | null, tenant: string | null): Identity => ({ provider: "google", subject: "s", email, tenant });
  expect(mayRegister(ms("a@x.com"), [{ kind: "address", value: "a@x.com" }])).toBe(true);
  expect(mayRegister(ms("a@x.com.evil"), [{ kind: "domain", value: "x.com" }])).toBe(false);
  expect(mayRegister(ms("a@sub.x.com"), [{ kind: "domain", value: "x.com" }])).toBe(false);
  expect(mayRegister(ms(null, "t1"), [{ kind: "tenant", value: "t1" }])).toBe(true);
  // A Google Workspace domain isn't an Entra tenant.
  expect(mayRegister(google("a@x.com", "t1"), [{ kind: "tenant", value: "t1" }])).toBe(false);
  expect(mayRegister(google(null, null), [{ kind: "anyone" }])).toBe(true);
  expect(mayRegister(ms("a@x.com"), [])).toBe(false);
});
