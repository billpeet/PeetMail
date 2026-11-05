// The routes end to end: real handlers, real id_tokens from a local issuer, a fresh database.
// Every response is checked against the shape of its example in protocol-fixtures/, which the
// Rust client tests against too.
import fs from "node:fs";
import { fileURLToPath } from "node:url";
import type { RequestEvent, RequestHandler } from "@sveltejs/kit";
import { afterAll, beforeAll, expect, test, vi } from "vitest";
import { parseKeyRequest, parseLinkRequest, parsePushRequest, parseRegisterRequest, parseSessionRequest } from "#lib/protocol.js";
import { resetService } from "#lib/server/http.js";
import { startIssuer, TEST_GOOGLE_APP, TEST_MS_APP, type Issuer } from "#lib/server/testing/issuer.js";
import * as devices from "./devices/+server.js";
import * as device from "./devices/[id]/+server.js";
import * as health from "./health/+server.js";
import * as identities from "./identities/+server.js";
import * as identity from "./identities/[id]/+server.js";
import * as items from "./items/+server.js";
import * as key from "./key/+server.js";
import * as session from "./session/+server.js";
import * as users from "./users/+server.js";
import * as me from "./users/me/+server.js";

const examples: Record<string, unknown> = JSON.parse(fs.readFileSync(fileURLToPath(new URL("../../../protocol-fixtures/examples.json", import.meta.url)), "utf8"));

let issuer: Issuer;

beforeAll(async () => {
  issuer = await startIssuer();
  Object.assign(process.env, {
    DATABASE_PATH: ":memory:",
    MIGRATIONS_DIR: fileURLToPath(new URL("../../../drizzle", import.meta.url)),
    ALLOW_REGISTER: "@fabrikam.io",
    MICROSOFT_CLIENT_IDS: TEST_MS_APP,
    GOOGLE_CLIENT_IDS: TEST_GOOGLE_APP,
    MICROSOFT_DISCOVERY_URL: issuer.discoveryUrls.microsoft,
    GOOGLE_DISCOVERY_URL: issuer.discoveryUrls.google,
  });
  resetService();
  vi.spyOn(console, "log").mockImplementation(() => {});
});
afterAll(() => issuer.close());

/** Calls a handler as SvelteKit would. */
async function call(handler: RequestHandler, init: { method?: string; path?: string; token?: string; body?: unknown; raw?: string; params?: Record<string, string> } = {}) {
  const url = new URL(init.path ?? "/v1/x", "http://sync.test");
  const headers = new Headers({ "content-type": "application/json" });
  if (init.token) headers.set("authorization", `Bearer ${init.token}`);
  const request = new Request(url, { method: init.method ?? "GET", headers, body: init.raw ?? (init.body === undefined ? undefined : JSON.stringify(init.body)) });
  const res = await handler({ request, url, params: init.params ?? {} } as unknown as RequestEvent);
  return { status: res.status, body: res.status === 204 ? undefined : await res.json() };
}

/** The same keys and value types, all the way down (arrays by their first element). */
function shape(v: unknown): unknown {
  if (Array.isArray(v)) return v.length ? [shape(v[0])] : [];
  if (v && typeof v === "object") return Object.fromEntries(Object.entries(v).sort(([a], [b]) => a.localeCompare(b)).map(([k, x]) => [k, shape(x)]));
  return v === null ? "null" : typeof v;
}
const like = (actual: unknown, example: string) => expect(shape(actual)).toEqual(shape(examples[example]));

test("the example requests are accepted as they are", () => {
  expect(parseSessionRequest(examples["session.request"])).toEqual(examples["session.request"]);
  expect(parseRegisterRequest(examples["users.request"])).toEqual(examples["users.request"]);
  expect(parseLinkRequest(examples["identities.request"])).toEqual(examples["identities.request"]);
  expect(parsePushRequest(examples["items.push.request"])).toEqual(examples["items.push.request"]);
  expect(parseKeyRequest(examples["key.request"])).toEqual(examples["key.request"]);
});

test("a whole sync account's life over the routes", async () => {
  like((await call(health.GET)).body, "health.response");

  const microsoft = await issuer.microsoft({ preferred_username: "priya@fabrikam.io" });
  const signIn = { provider: "microsoft", idToken: microsoft, deviceName: "Desk PC" };
  const none = await call(session.POST, { method: "POST", body: signIn });
  expect(none.status).toBe(404);
  like(none.body, "error.response");

  const { wrappedKey, kdfParams } = examples["users.request"] as { wrappedKey: string; kdfParams: object };
  const reg = await call(users.POST, { method: "POST", body: { ...signIn, wrappedKey, kdfParams } });
  expect(reg.status).toBe(200);
  like(reg.body, "session.response");
  const token: string = reg.body.deviceToken;

  like((await call(identities.GET, { token })).body, "identities.response");
  const linked = await call(identities.POST, { method: "POST", token, body: { provider: "google", idToken: await issuer.google() } });
  like(linked.body, "identity.response");

  const pushed = await call(items.PUT, {
    method: "PUT",
    token,
    body: { items: [{ id: "p0Yj8sX2n4kQ", blob: "QQ", deleted: false, baseSeq: null }, { id: "Zr7Tq1mW9cLe", blob: "Qg", deleted: false, baseSeq: null }] },
  });
  const conflicted = await call(items.PUT, { method: "PUT", token, body: { items: [{ id: "Zr7Tq1mW9cLe", blob: "Qw", deleted: true, baseSeq: 0 }] } });
  like({ results: [pushed.body.results[0], conflicted.body.results[0]] }, "items.push.response");

  const pulled = await call(items.GET, { path: "/v1/items?since=1&limit=5", token });
  like(pulled.body, "items.pull.response");
  expect(pulled.body.items.map((i: { id: string }) => i.id)).toEqual(["Zr7Tq1mW9cLe"]);

  const laptop = await call(session.POST, { method: "POST", body: { ...signIn, deviceName: "Laptop" } });
  const list = await call(devices.GET, { token });
  like(list.body, "devices.response");
  expect(list.body).toHaveLength(2);

  expect((await call(key.PUT, { method: "PUT", token, body: examples["key.request"] })).status).toBe(204);
  expect((await call(device.DELETE, { method: "DELETE", token, params: { id: String(laptop.body.deviceId) } })).status).toBe(204);
  expect((await call(devices.GET, { token: laptop.body.deviceToken })).body.error).toBe("device_revoked");
  expect((await call(identity.DELETE, { method: "DELETE", token, params: { id: String(linked.body.id) } })).status).toBe(204);

  expect((await call(me.DELETE, { method: "DELETE", token })).status).toBe(204);
  expect((await call(devices.GET, { token })).status).toBe(401);
});

test("bad requests get a clear error, not a crash", async () => {
  const bad = await call(session.POST, { method: "POST", raw: "{not json" });
  expect(bad).toEqual({ status: 400, body: { error: "bad_request", message: "The request body must be JSON." } });
  expect((await call(session.POST, { method: "POST", body: { provider: "yahoo", idToken: "x", deviceName: "d" } })).body.message).toMatch(/provider/);
  expect((await call(items.GET, { path: "/v1/items?since=-1", token: "pmd_x" })).status).toBe(401);
  expect((await call(devices.GET)).body.error).toBe("unauthorized");
  expect((await call(device.DELETE, { method: "DELETE", token: "pmd_x", params: { id: "abc" } })).status).toBe(401);
  const refused = await call(session.POST, { method: "POST", body: { provider: "microsoft", idToken: "not.a.jwt", deviceName: "d" } });
  expect(refused.status).toBe(401);
  // Registration is limited to fabrikam.io.
  const outsider = await issuer.microsoft({ oid: "o2", preferred_username: "eve@elsewhere.net" });
  const closed = await call(users.POST, { method: "POST", body: { provider: "microsoft", idToken: outsider, deviceName: "d", wrappedKey: "AA", kdfParams: {} } });
  expect(closed.body.error).toBe("registration_closed");
});
