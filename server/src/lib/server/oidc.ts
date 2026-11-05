// Checks a mail account's OpenID id_token (docs/SYNC.md → Several mail accounts): signed by
// the provider, issued to one of PeetMail's apps, and fresh. Gives back who it is.
import { createRemoteJWKSet, jwtVerify, type JWTPayload } from "jose";
import type { Provider } from "#lib/protocol.js";
import type { Config } from "./config.js";
import { SyncError } from "./errors.js";

export type Identity = {
  provider: Provider;
  /** Stable for the person. Microsoft: `<tid>:<oid>` (Entra's `sub` differs per app). Google: `sub`. */
  subject: string;
  /** For telling users apart and for ALLOW_REGISTER. Microsoft: the sign-in name; Google: a verified address. */
  email: string | null;
  /** Microsoft: the Entra tenant. Google: the Workspace domain (`hd`). */
  tenant: string | null;
};

export type Verifier = (provider: Provider, idToken: string) => Promise<Identity>;

/** How old an id_token may be: one from a sign-in or refresh a few minutes ago. */
export const MAX_TOKEN_AGE_SECS = 10 * 60;
const CLOCK_TOLERANCE_SECS = 60;
const DISCOVERY_TTL_MS = 24 * 60 * 60 * 1000;

type Discovery = { issuer: string; jwks: ReturnType<typeof createRemoteJWKSet>; fetchedAt: number };

const invalid = (why: string) => new SyncError(401, "invalid_token", `The sign-in token was refused: ${why}.`);

export function createVerifier(config: Config, now: () => number = Date.now): Verifier {
  const discovered = new Map<Provider, Discovery>();

  async function discover(provider: Provider): Promise<Discovery> {
    const cached = discovered.get(provider);
    if (cached && now() - cached.fetchedAt < DISCOVERY_TTL_MS) return cached;
    let doc: { issuer?: unknown; jwks_uri?: unknown };
    try {
      const res = await fetch(config.discoveryUrls[provider]);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      doc = await res.json();
    } catch (e) {
      console.error(`couldn't fetch the ${provider} OpenID configuration:`, e);
      throw new SyncError(502, "provider_unavailable", `The server couldn't reach ${provider === "microsoft" ? "Microsoft" : "Google"} to check the sign-in. Try again shortly.`);
    }
    if (typeof doc.issuer !== "string" || typeof doc.jwks_uri !== "string") {
      throw new SyncError(502, "provider_unavailable", `The ${provider} OpenID configuration is missing its issuer or keys.`);
    }
    const d = { issuer: doc.issuer, jwks: createRemoteJWKSet(new URL(doc.jwks_uri)), fetchedAt: now() };
    discovered.set(provider, d);
    return d;
  }

  return async (provider, idToken) => {
    const audience = config.clientIds[provider];
    if (!audience.length) {
      throw new SyncError(403, "provider_disabled", `This server doesn't accept ${provider === "microsoft" ? "Microsoft" : "Google"} accounts.`);
    }
    const { issuer, jwks } = await discover(provider);
    let payload: JWTPayload;
    try {
      ({ payload } = await jwtVerify(idToken, jwks, {
        audience,
        algorithms: ["RS256"],
        clockTolerance: CLOCK_TOLERANCE_SECS,
        currentDate: new Date(now()),
        requiredClaims: ["iss", "sub", "aud", "exp", "iat"],
      }));
    } catch (e) {
      const code = (e as { code?: string }).code ?? "";
      const why =
        code === "ERR_JWT_EXPIRED" ? "it has expired"
        : code === "ERR_JWT_CLAIM_VALIDATION_FAILED" ? `its ${(e as { claim?: string }).claim ?? "claims"} didn't match`
        : code === "ERR_JWKS_NO_MATCHING_KEY" ? "it isn't signed with the provider's keys"
        : "it isn't a valid id_token";
      throw invalid(why);
    }
    if (now() / 1000 - (payload.iat as number) > MAX_TOKEN_AGE_SECS + CLOCK_TOLERANCE_SECS) {
      throw invalid("it is too old; sign in again");
    }
    return provider === "microsoft" ? microsoft(payload, issuer) : google(payload, issuer);
  };
}

const str = (v: unknown) => (typeof v === "string" && v ? v : null);

function microsoft(p: JWTPayload, issuer: string): Identity {
  const tid = str(p.tid);
  const oid = str(p.oid);
  if (!tid || !oid) throw invalid("it has no tenant or object id");
  // Multi-tenant discovery gives the issuer as a template: https://login.microsoftonline.com/{tenantid}/v2.0.
  if (p.iss !== issuer.replaceAll("{tenantid}", tid)) throw invalid("its issuer didn't match its tenant");
  // preferred_username is the UPN for work accounts (its domain is one the tenant verified) and the
  // address for personal ones. The `email` claim isn't verified by Entra, so it's never used.
  const email = str(p.preferred_username)?.toLowerCase() ?? null;
  return { provider: "microsoft", subject: `${tid}:${oid}`, email, tenant: tid.toLowerCase() };
}

function google(p: JWTPayload, issuer: string): Identity {
  // Google's tokens say either form.
  const issuers = [issuer, issuer.replace(/^https:\/\//, "")];
  if (!issuers.includes(p.iss as string)) throw invalid("its issuer isn't Google");
  const email = p.email_verified === true ? (str(p.email)?.toLowerCase() ?? null) : null;
  return { provider: "google", subject: p.sub as string, email, tenant: str(p.hd)?.toLowerCase() ?? null };
}
