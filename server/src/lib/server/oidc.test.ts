import { generateKeyPair } from "jose";
import { afterAll, beforeAll, describe, expect, test } from "vitest";
import type { Config } from "./config.js";
import { createVerifier, MAX_TOKEN_AGE_SECS } from "./oidc.js";
import { startIssuer, TENANT, TEST_GOOGLE_APP, TEST_MS_APP, type Issuer } from "./testing/issuer.js";

let issuer: Issuer;
let config: Config;

beforeAll(async () => {
  issuer = await startIssuer();
  config = {
    databasePath: ":memory:",
    migrationsDir: "",
    allowRegister: [],
    clientIds: { microsoft: [TEST_MS_APP], google: [TEST_GOOGLE_APP] },
    discoveryUrls: issuer.discoveryUrls,
  };
});
afterAll(() => issuer.close());

const refused = (p: Promise<unknown>, why: RegExp) => expect(p).rejects.toMatchObject({ status: 401, code: "invalid_token", message: expect.stringMatching(why) });

describe("Microsoft", () => {
  test("a work account is its tenant and object id, with the sign-in name", async () => {
    const verify = createVerifier(config);
    expect(await verify("microsoft", await issuer.microsoft({ oid: "abc", preferred_username: "Priya@Fabrikam.io" }))).toEqual({
      provider: "microsoft",
      subject: `${TENANT}:abc`,
      email: "priya@fabrikam.io",
      tenant: TENANT,
    });
  });

  test("the same person through two apps is one identity, though Entra's sub differs", async () => {
    const verify = createVerifier({ ...config, clientIds: { ...config.clientIds, microsoft: [TEST_MS_APP, "own-app"] } });
    const a = await verify("microsoft", await issuer.microsoft({ oid: "abc", sub: "sub-for-app-1" }));
    const b = await verify("microsoft", await issuer.microsoft({ oid: "abc", sub: "sub-for-app-2", aud: "own-app" }));
    expect(a.subject).toBe(b.subject);
  });

  test("a token for another app, tenant or issuer is refused", async () => {
    const verify = createVerifier(config);
    await refused(verify("microsoft", await issuer.microsoft({ aud: "someone-elses-app" })), /aud/);
    // Signed for one tenant but claiming another: the issuer must be the tenant's own.
    const tok = await issuer.microsoft({ tid: "00000000-0000-0000-0000-000000000001" });
    await refused(verify("microsoft", await issuer.sign({ ...JSON.parse(Buffer.from(tok.split(".")[1], "base64url").toString()), tid: TENANT })), /issuer/);
    await refused(verify("microsoft", await issuer.microsoft({ oid: "" })), /object id/);
  });

  test("the email claim is never trusted", async () => {
    const verify = createVerifier(config);
    const id = await verify("microsoft", await issuer.microsoft({ preferred_username: undefined, email: "ceo@contoso.com" }));
    expect(id.email).toBeNull();
  });
});

describe("Google", () => {
  test("a verified address comes through; an unverified one doesn't", async () => {
    const verify = createVerifier(config);
    expect(await verify("google", await issuer.google({ hd: "Example.com" }))).toEqual({
      provider: "google",
      subject: "1080001",
      email: "sam@example.com",
      tenant: "example.com",
    });
    expect((await verify("google", await issuer.google({ email_verified: false }))).email).toBeNull();
  });

  test("Microsoft's tokens aren't Google's", async () => {
    const verify = createVerifier({ ...config, clientIds: { microsoft: [TEST_MS_APP], google: [TEST_MS_APP] } });
    await refused(verify("google", await issuer.microsoft()), /issuer/);
  });

  test("off unless the server lists a Google app", async () => {
    const verify = createVerifier({ ...config, clientIds: { ...config.clientIds, google: [] } });
    await expect(verify("google", await issuer.google())).rejects.toMatchObject({ status: 403, code: "provider_disabled" });
  });
});

test("expired, stale, forged and malformed tokens are refused", async () => {
  const verify = createVerifier(config);
  const old = Math.floor(Date.now() / 1000) - 2 * 3600;
  await refused(verify("microsoft", await issuer.microsoft({ iat: old, exp: old + 3600 })), /expired/);
  // Still valid but from long ago: an id_token is proof of a recent sign-in.
  const stale = Math.floor(Date.now() / 1000) - MAX_TOKEN_AGE_SECS - 120;
  await refused(verify("microsoft", await issuer.microsoft({ iat: stale })), /too old/);
  const { privateKey } = await generateKeyPair("RS256");
  await refused(verify("microsoft", await issuer.sign({ iss: "x" }, { key: privateKey })), /valid/);
  await refused(verify("microsoft", await issuer.microsoft({}).then((t) => t.slice(0, -4) + "AAAA")), /valid/);
  await refused(verify("microsoft", await issuer.sign({}, { key: privateKey, kid: "other" })), /keys/);
  await refused(verify("microsoft", "not.a.jwt"), /valid/);
  const none = `${Buffer.from('{"alg":"none"}').toString("base64url")}.${Buffer.from(JSON.stringify({ aud: TEST_MS_APP })).toString("base64url")}.`;
  await refused(verify("microsoft", none), /valid/);
});

test("an unreachable provider is a 502, not a refusal", async () => {
  const verify = createVerifier({ ...config, discoveryUrls: { ...config.discoveryUrls, microsoft: "http://127.0.0.1:9/nothing" } });
  const err = console.error;
  console.error = () => {};
  try {
    await expect(verify("microsoft", await issuer.microsoft())).rejects.toMatchObject({ status: 502, code: "provider_unavailable" });
  } finally {
    console.error = err;
  }
});
