// The server's settings, from environment variables (server/README.md lists them).
import path from "node:path";
import type { Provider } from "#lib/protocol.js";

/** Who may create a sync account. Linking more mail accounts to an existing one is always allowed. */
export type RegisterRule =
  | { kind: "anyone" }
  | { kind: "address"; value: string }
  | { kind: "domain"; value: string }
  | { kind: "tenant"; value: string };

export type Config = {
  databasePath: string;
  migrationsDir: string;
  allowRegister: RegisterRule[];
  /** Accepted id_token audiences: the OAuth apps PeetMail signs in with. */
  clientIds: Record<Provider, string[]>;
  /** OpenID discovery documents. Overridable so tests can stand in a fake provider. */
  discoveryUrls: Record<Provider, string>;
};

/** PeetMail's built-in Microsoft app (docs/DISTRIBUTION.md, stage 5). */
export const PEETMAIL_MICROSOFT_CLIENT_ID = "04e0699b-c2f4-46f4-aa06-918aff940115";

export const DEFAULT_DISCOVERY: Record<Provider, string> = {
  microsoft: "https://login.microsoftonline.com/common/v2.0/.well-known/openid-configuration",
  google: "https://accounts.google.com/.well-known/openid-configuration",
};

export class ConfigError extends Error {}

const list = (v: string | undefined) => (v ?? "").split(/[\s,]+/).filter(Boolean);

export function parseRegisterRules(raw: string | undefined): RegisterRule[] {
  if (raw === undefined || !raw.trim()) {
    throw new ConfigError(
      "ALLOW_REGISTER isn't set. Set it to who may create a sync account: addresses, @domain, tenant:<Entra tenant id>, " +
        '"*" for anyone, or "none" to let only existing users in.',
    );
  }
  const rules: RegisterRule[] = [];
  for (const entry of list(raw.toLowerCase())) {
    if (entry === "none") continue;
    if (entry === "*") rules.push({ kind: "anyone" });
    else if (entry.startsWith("tenant:") && entry.length > 7) rules.push({ kind: "tenant", value: entry.slice(7) });
    else if (entry.startsWith("@") && entry.length > 1) rules.push({ kind: "domain", value: entry.slice(1) });
    else if (/^[^@\s]+@[^@\s]+$/.test(entry)) rules.push({ kind: "address", value: entry });
    else throw new ConfigError(`ALLOW_REGISTER: "${entry}" isn't an address, @domain, tenant:<id>, * or none.`);
  }
  return rules;
}

export function loadConfig(env: Record<string, string | undefined> = process.env): Config {
  const microsoft = env.MICROSOFT_CLIENT_IDS === undefined ? [PEETMAIL_MICROSOFT_CLIENT_ID] : list(env.MICROSOFT_CLIENT_IDS);
  // PeetMail's built-in Google app isn't registered yet, so Google sign-in is off by default.
  const google = list(env.GOOGLE_CLIENT_IDS);
  return {
    databasePath: env.DATABASE_PATH || path.resolve("data", "sync.db"),
    migrationsDir: env.MIGRATIONS_DIR || path.resolve("drizzle"),
    allowRegister: parseRegisterRules(env.ALLOW_REGISTER),
    clientIds: { microsoft, google },
    discoveryUrls: {
      microsoft: env.MICROSOFT_DISCOVERY_URL || DEFAULT_DISCOVERY.microsoft,
      google: env.GOOGLE_DISCOVERY_URL || DEFAULT_DISCOVERY.google,
    },
  };
}
