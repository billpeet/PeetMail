import fs from "node:fs";
import path from "node:path";
import Database from "better-sqlite3";
import { drizzle } from "drizzle-orm/better-sqlite3";
import { migrate } from "drizzle-orm/better-sqlite3/migrator";
import * as schema from "./schema.js";

/** Opens (creating if needed) and migrates the database. `:memory:` for tests. */
export function openDb(file: string, migrationsDir: string) {
  if (file !== ":memory:") fs.mkdirSync(path.dirname(file), { recursive: true });
  const sqlite = new Database(file);
  sqlite.pragma("journal_mode = WAL");
  sqlite.pragma("foreign_keys = ON");
  sqlite.pragma("busy_timeout = 5000");
  const db = drizzle({ client: sqlite, schema });
  migrate(db, { migrationsFolder: migrationsDir });
  return db;
}

export type Db = ReturnType<typeof openDb>;
