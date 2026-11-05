// The real settings sync server (server/), built and run for E2E tests, trusting the fake
// Graph's id_tokens (its discovery document and keys). Each test gets its own server and
// database.

import { spawn, spawnSync, type ChildProcess } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { freePort, ROOT } from "./app.ts";
import { CLIENT_ID } from "./fake-graph.ts";

const SERVER = join(ROOT, "server");
const BUILD = join(SERVER, "build");
const STAMP = join(SERVER, "build", ".harness-stamp");

function newestMtime(path: string): number {
  if (!existsSync(path)) return 0;
  const st = statSync(path);
  if (!st.isDirectory()) return st.mtimeMs;
  return Math.max(0, ...readdirSync(path).map((n) => newestMtime(join(path, n))));
}

/** Builds the server (pnpm --filter peetmail-sync build) if its sources changed since. */
export function ensureSyncServerBuilt({ log = console.log }: { log?: (msg: string) => void } = {}) {
  const sources = ["src", "drizzle", "package.json", "vite.config.js"].map((p) => newestMtime(join(SERVER, p)));
  if (existsSync(STAMP) && statSync(STAMP).mtimeMs >= Math.max(...sources)) return;
  log("building the sync server (pnpm --filter peetmail-sync build)…");
  const r = spawnSync("pnpm --filter peetmail-sync build", { cwd: ROOT, stdio: "inherit", shell: true });
  if (r.status !== 0) throw new Error("sync server build failed");
  writeFileSync(STAMP, new Date().toISOString());
}

export type SyncServer = {
  url: string;
  /** The server's SQLite file (with its WAL), as bytes: for the plaintext canary. */
  databaseBytes: () => Buffer;
  log: () => string;
  close: () => Promise<void>;
};

/** Starts a server that trusts tokens from the fake Graph at `authorityUrl` (…/oauth). */
export async function startSyncServer({ authorityUrl, allowRegister = "*" }: { authorityUrl: string; allowRegister?: string }): Promise<SyncServer> {
  ensureSyncServerBuilt();
  const dir = await mkdtemp(join(tmpdir(), "peetmail-sync-test-"));
  const port = await freePort();
  const dbPath = join(dir, "sync.db");
  let output = "";
  const proc: ChildProcess = spawn(process.execPath, [BUILD], {
    cwd: SERVER,
    env: {
      ...process.env,
      PORT: String(port),
      HOST: "127.0.0.1",
      DATABASE_PATH: dbPath,
      MIGRATIONS_DIR: join(SERVER, "drizzle"),
      ALLOW_REGISTER: allowRegister,
      MICROSOFT_CLIENT_IDS: CLIENT_ID,
      MICROSOFT_DISCOVERY_URL: `${authorityUrl}/v2.0/.well-known/openid-configuration`,
      GOOGLE_CLIENT_IDS: "",
    },
    stdio: ["ignore", "pipe", "pipe"],
  });
  proc.stdout?.on("data", (d) => (output += d));
  proc.stderr?.on("data", (d) => (output += d));
  const url = `http://127.0.0.1:${port}`;
  const deadline = Date.now() + 20_000;
  for (;;) {
    if (proc.exitCode !== null) throw new Error(`sync server exited: ${output}`);
    try {
      if ((await fetch(`${url}/v1/health`)).ok) break;
    } catch {}
    if (Date.now() > deadline) throw new Error(`sync server didn't start: ${output}`);
    await new Promise((r) => setTimeout(r, 100));
  }
  return {
    url,
    databaseBytes: () => Buffer.concat([dbPath, `${dbPath}-wal`].filter(existsSync).map((f) => readFileSync(f))),
    log: () => output,
    close: async () => {
      proc.kill();
      await new Promise((r) => (proc.exitCode !== null ? r(null) : proc.once("exit", r)));
      await rm(dir, { recursive: true, force: true }).catch(() => {});
    },
  };
}
