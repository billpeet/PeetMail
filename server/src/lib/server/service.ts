// What each call does (docs/SYNC.md → Protocol), independent of HTTP so it can be tested
// directly. The server never sees what an item holds: it stores ciphertext under opaque ids and
// does compare-and-swap on versions. Merging is the client's job.
import { createHash, randomBytes } from "node:crypto";
import { and, asc, count, eq, gt, isNull } from "drizzle-orm";
import {
  LIMITS,
  type Device,
  type Item,
  type KeyRequest,
  type LinkedAccount,
  type LinkRequest,
  type Provider,
  type Pull,
  type Push,
  type PushItem,
  type PushResult,
  type RegisterRequest,
  type Session,
  type SessionRequest,
} from "#lib/protocol.js";
import type { Config, RegisterRule } from "./config.js";
import type { Db } from "./db.js";
import { SyncError } from "./errors.js";
import type { Identity, Verifier } from "./oidc.js";
import { devices, identities, items, users } from "./schema.js";

/** A signed-in device. */
export type Auth = { userId: number; deviceId: number };

const TOKEN_PREFIX = "pmd_";
/** Don't rewrite last_seen_at on every request. */
const SEEN_EVERY_MS = 5 * 60 * 1000;

const hash = (token: string) => createHash("sha256").update(token).digest("hex");
const nowIso = (now: number) => new Date(now).toISOString();

/** Whether `identity` may create a sync account under the server's ALLOW_REGISTER rules. */
export function mayRegister(identity: Identity, rules: RegisterRule[]): boolean {
  const email = identity.email;
  return rules.some((r) => {
    switch (r.kind) {
      case "anyone":
        return true;
      case "address":
        return email === r.value;
      case "domain":
        return !!email && email.endsWith(`@${r.value}`);
      case "tenant":
        return identity.provider === "microsoft" && identity.tenant === r.value;
    }
  });
}

export function createService(db: Db, verify: Verifier, config: Config, now: () => number = Date.now) {
  function newDevice(userId: number, name: string) {
    const token = TOKEN_PREFIX + randomBytes(32).toString("base64url");
    const at = nowIso(now());
    const row = db
      .insert(devices)
      .values({ userId, name, tokenHash: hash(token), createdAt: at, lastSeenAt: at })
      .returning({ id: devices.id })
      .get();
    return { deviceId: row.id, deviceToken: token };
  }

  function session(userId: number, deviceName: string): Session {
    const user = db.select().from(users).where(eq(users.id, userId)).get()!;
    return { ...newDevice(userId, deviceName), wrappedKey: user.wrappedKey, kdfParams: JSON.parse(user.kdfParams) };
  }

  const findIdentity = (identity: Identity) =>
    db
      .select()
      .from(identities)
      .where(and(eq(identities.kind, identity.provider), eq(identities.subject, identity.subject)))
      .get();

  return {
    /** Signs a device in with a linked mail account. */
    async signIn(req: SessionRequest): Promise<Session> {
      const identity = await verify(req.provider, req.idToken);
      const linked = findIdentity(identity);
      if (!linked) throw new SyncError(404, "no_user", "This mail account isn't linked to a sync account on this server.");
      return db.transaction(() => session(linked.userId, req.deviceName));
    },

    /** Creates a sync account with this mail account as its first identity. */
    async register(req: RegisterRequest): Promise<Session> {
      const identity = await verify(req.provider, req.idToken);
      if (findIdentity(identity)) {
        throw new SyncError(409, "already_registered", "This mail account already has a sync account. Sign in instead.");
      }
      if (!mayRegister(identity, config.allowRegister)) {
        throw new SyncError(403, "registration_closed", "This server doesn't let this account create a sync account.");
      }
      return db.transaction((tx) => {
        const user = tx
          .insert(users)
          .values({ wrappedKey: req.wrappedKey, kdfParams: JSON.stringify(req.kdfParams), createdAt: nowIso(now()) })
          .returning({ id: users.id })
          .get();
        tx.insert(identities)
          .values({ userId: user.id, kind: identity.provider, subject: identity.subject, emailHint: identity.email, linkedAt: nowIso(now()) })
          .run();
        console.log(`new sync account ${user.id} for ${identity.email ?? `a ${identity.provider} account`}`);
        return session(user.id, req.deviceName);
      });
    },

    /** The device behind a bearer token. */
    authenticate(header: string | null): Auth {
      const token = header?.match(/^Bearer\s+(\S+)$/i)?.[1];
      if (!token?.startsWith(TOKEN_PREFIX)) throw new SyncError(401, "unauthorized", "Sign in to sync first.");
      const device = db.select().from(devices).where(eq(devices.tokenHash, hash(token))).get();
      if (!device) throw new SyncError(401, "unauthorized", "This device isn't signed in to sync.");
      if (device.revokedAt) throw new SyncError(401, "device_revoked", "This device was signed out of sync.");
      if (now() - Date.parse(device.lastSeenAt) > SEEN_EVERY_MS) {
        db.update(devices).set({ lastSeenAt: nowIso(now()) }).where(eq(devices.id, device.id)).run();
      }
      return { userId: device.userId, deviceId: device.id };
    },

    async linkIdentity(auth: Auth, req: LinkRequest): Promise<LinkedAccount> {
      const identity = await verify(req.provider, req.idToken);
      const existing = findIdentity(identity);
      if (existing && existing.userId !== auth.userId) {
        throw new SyncError(409, "linked_elsewhere", "This mail account is linked to a different sync account.");
      }
      const row =
        existing ??
        db
          .insert(identities)
          .values({ userId: auth.userId, kind: identity.provider, subject: identity.subject, emailHint: identity.email, linkedAt: nowIso(now()) })
          .returning()
          .get();
      return linkedAccount(row);
    },

    listIdentities(auth: Auth): LinkedAccount[] {
      return db.select().from(identities).where(eq(identities.userId, auth.userId)).orderBy(asc(identities.id)).all().map(linkedAccount);
    },

    unlinkIdentity(auth: Auth, id: number): void {
      db.transaction((tx) => {
        const mine = tx.select({ id: identities.id }).from(identities).where(eq(identities.userId, auth.userId)).all();
        if (!mine.some((r) => r.id === id)) throw new SyncError(404, "not_found", "No such linked account.");
        if (mine.length === 1) {
          throw new SyncError(409, "last_identity", "This is the only mail account that signs in to this sync account, so it can't be unlinked.");
        }
        tx.delete(identities).where(eq(identities.id, id)).run();
      });
    },

    /** Items changed after `since`, oldest first. */
    pull(auth: Auth, since: number, limit: number): Pull {
      const rows = db
        .select({ id: items.itemId, seq: items.seq, blob: items.blob, deleted: items.deleted })
        .from(items)
        .where(and(eq(items.userId, auth.userId), gt(items.seq, since)))
        .orderBy(asc(items.seq))
        .limit(limit + 1)
        .all();
      const page: Item[] = rows.slice(0, limit);
      return { items: page, cursor: page.at(-1)?.seq ?? since, more: rows.length > limit };
    },

    /**
     * Compare-and-swap per item: a change made on the version the server has (or on a new
     * item) is stored under the next version; any other gets the server's copy back to merge.
     */
    push(auth: Auth, changes: PushItem[]): Push {
      return db.transaction((tx) => {
        const current = new Map(
          tx
            .select({ id: items.itemId, seq: items.seq, blob: items.blob, deleted: items.deleted })
            .from(items)
            .where(eq(items.userId, auth.userId))
            .all()
            .map((r) => [r.id, r]),
        );
        const added = changes.filter((c) => !current.has(c.id)).length;
        if (current.size + added > LIMITS.userItems) {
          throw new SyncError(413, "too_many_items", `A sync account can hold at most ${LIMITS.userItems} items.`);
        }
        let seq = tx.select({ seq: users.seq }).from(users).where(eq(users.id, auth.userId)).get()!.seq;
        const at = nowIso(now());
        const results = changes.map((c): PushResult => {
          const have = current.get(c.id);
          if (have && have.seq !== c.baseSeq) {
            return { id: c.id, status: "conflict", seq: have.seq, blob: have.blob, deleted: have.deleted };
          }
          seq += 1;
          tx.insert(items)
            .values({ userId: auth.userId, itemId: c.id, seq, blob: c.blob, deleted: c.deleted, updatedAt: at })
            .onConflictDoUpdate({ target: [items.userId, items.itemId], set: { seq, blob: c.blob, deleted: c.deleted, updatedAt: at } })
            .run();
          return { id: c.id, status: "ok", seq };
        });
        tx.update(users).set({ seq }).where(eq(users.id, auth.userId)).run();
        return { results };
      });
    },

    listDevices(auth: Auth): Device[] {
      return db
        .select()
        .from(devices)
        .where(and(eq(devices.userId, auth.userId), isNull(devices.revokedAt)))
        .orderBy(asc(devices.id))
        .all()
        .map((d) => ({ id: d.id, name: d.name, createdAt: d.createdAt, lastSeenAt: d.lastSeenAt, current: d.id === auth.deviceId }));
    },

    /** Signs a device out of sync (this one included). */
    revokeDevice(auth: Auth, id: number): void {
      const done = db
        .update(devices)
        .set({ revokedAt: nowIso(now()) })
        .where(and(eq(devices.id, id), eq(devices.userId, auth.userId), isNull(devices.revokedAt)))
        .run();
      if (!done.changes) throw new SyncError(404, "not_found", "No such device.");
    },

    /** A new passphrase: the same master key, wrapped again. */
    setKey(auth: Auth, req: KeyRequest): void {
      db.update(users).set({ wrappedKey: req.wrappedKey, kdfParams: JSON.stringify(req.kdfParams) }).where(eq(users.id, auth.userId)).run();
    },

    /** Deletes the sync account: identities, devices and items go with it. */
    deleteUser(auth: Auth): void {
      db.delete(users).where(eq(users.id, auth.userId)).run();
      console.log(`deleted sync account ${auth.userId}`);
    },

    /** How many users, for the startup log. */
    userCount(): number {
      return db.select({ n: count() }).from(users).get()!.n;
    },
  };
}

export type Service = ReturnType<typeof createService>;

function linkedAccount(r: typeof identities.$inferSelect): LinkedAccount {
  return { id: r.id, provider: r.kind as Provider, email: r.emailHint, linkedAt: r.linkedAt };
}
