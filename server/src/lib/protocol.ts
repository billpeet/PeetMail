// The sync protocol's request and response bodies (docs/SYNC.md → Protocol), with checks for
// what clients send. JSON, camelCase. The Rust client has its own copy of these types; both
// sides test against the examples in protocol-fixtures/.
import { badRequest } from "#lib/server/errors.js";

export const PROTOCOL_VERSION = 1;

export type Provider = "microsoft" | "google";

export type Health = { ok: true; protocol: number; version: string };

export type SessionRequest = { provider: Provider; idToken: string; deviceName: string };
/** The key material is opaque to the server: a wrapped key and the KDF's parameters. */
export type RegisterRequest = SessionRequest & KeyRequest;
export type Session = { deviceId: number; deviceToken: string; wrappedKey: string; kdfParams: Record<string, unknown> };

export type LinkRequest = { provider: Provider; idToken: string };
export type LinkedAccount = { id: number; provider: Provider; email: string | null; linkedAt: string };

export type Item = { id: string; seq: number; blob: string; deleted: boolean };
export type Pull = { items: Item[]; cursor: number; more: boolean };
/** `baseSeq`: the version this change was made on; null for an item new to this device. */
export type PushItem = { id: string; blob: string; deleted: boolean; baseSeq: number | null };
export type PushRequest = { items: PushItem[] };
export type PushResult =
  | { id: string; status: "ok"; seq: number }
  | { id: string; status: "conflict"; seq: number; blob: string; deleted: boolean };
export type Push = { results: PushResult[] };

export type Device = { id: number; name: string; createdAt: string; lastSeenAt: string; current: boolean };
export type KeyRequest = { wrappedKey: string; kdfParams: Record<string, unknown> };

export type ErrorBody = { error: string; message: string };

export const LIMITS = {
  /** Items in one push. */
  pushItems: 500,
  /** Items in one pull page. */
  pullItems: 1000,
  /** Items a user may have (tombstones included). */
  userItems: 10_000,
  /** Characters in one blob (base64 of the ciphertext). */
  blob: 256 * 1024,
  itemId: 128,
  deviceName: 100,
  wrappedKey: 4096,
  kdfParams: 1024,
  idToken: 16 * 1024,
};

// ---------- checks ----------

type Obj = Record<string, unknown>;

function object(v: unknown, what = "The request body"): Obj {
  if (typeof v !== "object" || v === null || Array.isArray(v)) throw badRequest(`${what} must be a JSON object.`);
  return v as Obj;
}

function string(o: Obj, key: string, max: number, pattern?: RegExp): string {
  const v = o[key];
  if (typeof v !== "string" || !v.length) throw badRequest(`"${key}" must be a non-empty string.`);
  if (v.length > max) throw badRequest(`"${key}" is too long (at most ${max} characters).`);
  if (pattern && !pattern.test(v)) throw badRequest(`"${key}" has characters it can't contain.`);
  return v;
}

function provider(o: Obj): Provider {
  const v = o.provider;
  if (v !== "microsoft" && v !== "google") throw badRequest(`"provider" must be "microsoft" or "google".`);
  return v;
}

function kdfParams(o: Obj): Record<string, unknown> {
  const v = object(o.kdfParams, `"kdfParams"`);
  if (JSON.stringify(v).length > LIMITS.kdfParams) throw badRequest(`"kdfParams" is too large.`);
  return v;
}

const ITEM_ID = /^[A-Za-z0-9_-]+$/;
const BASE64 = /^[A-Za-z0-9+/_=-]+$/;
const NO_CONTROL_CHARS = /^[^\p{Cc}]+$/u;

export function parseSessionRequest(body: unknown): SessionRequest {
  const o = object(body);
  return {
    provider: provider(o),
    idToken: string(o, "idToken", LIMITS.idToken),
    deviceName: string(o, "deviceName", LIMITS.deviceName, NO_CONTROL_CHARS).trim() || "Device",
  };
}

export function parseRegisterRequest(body: unknown): RegisterRequest {
  const o = object(body);
  return { ...parseSessionRequest(o), ...parseKeyRequest(o) };
}

export function parseLinkRequest(body: unknown): LinkRequest {
  const o = object(body);
  return { provider: provider(o), idToken: string(o, "idToken", LIMITS.idToken) };
}

export function parseKeyRequest(body: unknown): KeyRequest {
  const o = object(body);
  return { wrappedKey: string(o, "wrappedKey", LIMITS.wrappedKey, BASE64), kdfParams: kdfParams(o) };
}

export function parsePushRequest(body: unknown): PushRequest {
  const o = object(body);
  if (!Array.isArray(o.items)) throw badRequest(`"items" must be a list.`);
  if (o.items.length > LIMITS.pushItems) throw badRequest(`At most ${LIMITS.pushItems} items can go in one push.`);
  const seen = new Set<string>();
  const items = o.items.map((raw, i): PushItem => {
    const it = object(raw, `items[${i}]`);
    const id = string(it, "id", LIMITS.itemId, ITEM_ID);
    if (seen.has(id)) throw badRequest(`Item ${id} is in the push twice.`);
    seen.add(id);
    const baseSeq = it.baseSeq ?? null;
    if (baseSeq !== null && !(typeof baseSeq === "number" && Number.isSafeInteger(baseSeq) && baseSeq >= 0)) {
      throw badRequest(`"baseSeq" must be a whole number or null.`);
    }
    const deleted = it.deleted ?? false;
    if (typeof deleted !== "boolean") throw badRequest(`"deleted" must be true or false.`);
    return { id, blob: string(it, "blob", LIMITS.blob, BASE64), deleted, baseSeq };
  });
  return { items };
}

/** A whole number from a query parameter, capped at `max`. */
export function parseCount(raw: string | null, fallback: number, max: number, name: string): number {
  if (raw === null || raw === "") return fallback;
  const n = Number(raw);
  if (!Number.isSafeInteger(n) || n < 0) throw badRequest(`"${name}" must be a whole number.`);
  return Math.min(n, max);
}
