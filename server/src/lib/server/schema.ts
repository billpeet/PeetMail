// The sync server's tables (docs/SYNC.md → Data model). Item blobs are ciphertext and item ids
// are HMACs: nothing here says what a user's settings are.
import { sql } from "drizzle-orm";
import { index, integer, primaryKey, sqliteTable, text, uniqueIndex } from "drizzle-orm/sqlite-core";

const now = sql`(strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))`;

export const users = sqliteTable("users", {
  id: integer("id").primaryKey({ autoIncrement: true }),
  createdAt: text("created_at").notNull().default(now),
  /** The master key, wrapped with the passphrase-derived key. Opaque to the server. */
  wrappedKey: text("wrapped_key").notNull(),
  /** The KDF's parameters (JSON), so devices can derive the same key. Opaque to the server. */
  kdfParams: text("kdf_params").notNull(),
  /** The last item version handed out; each write takes the next. */
  seq: integer("seq").notNull().default(0),
});

/** Mail accounts that sign in to a user. */
export const identities = sqliteTable(
  "identities",
  {
    id: integer("id").primaryKey({ autoIncrement: true }),
    userId: integer("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    /** "microsoft" or "google". */
    kind: text("kind").notNull(),
    /** Microsoft: `<tid>:<oid>`. Google: `sub`. */
    subject: text("subject").notNull(),
    /** The account's address, so the person running the server can tell users apart. */
    emailHint: text("email_hint"),
    linkedAt: text("linked_at").notNull().default(now),
  },
  (t) => [uniqueIndex("identities_kind_subject").on(t.kind, t.subject), index("identities_user").on(t.userId)],
);

export const devices = sqliteTable(
  "devices",
  {
    id: integer("id").primaryKey({ autoIncrement: true }),
    userId: integer("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    /** SHA-256 of the device token; the token itself is never stored. */
    tokenHash: text("token_hash").notNull(),
    createdAt: text("created_at").notNull().default(now),
    lastSeenAt: text("last_seen_at").notNull().default(now),
    revokedAt: text("revoked_at"),
  },
  (t) => [uniqueIndex("devices_token_hash").on(t.tokenHash), index("devices_user").on(t.userId)],
);

export const items = sqliteTable(
  "items",
  {
    userId: integer("user_id")
      .notNull()
      .references(() => users.id, { onDelete: "cascade" }),
    itemId: text("item_id").notNull(),
    seq: integer("seq").notNull(),
    blob: text("blob").notNull(),
    deleted: integer("deleted", { mode: "boolean" }).notNull().default(false),
    updatedAt: text("updated_at").notNull().default(now),
  },
  (t) => [primaryKey({ columns: [t.userId, t.itemId] }), index("items_user_seq").on(t.userId, t.seq)],
);
