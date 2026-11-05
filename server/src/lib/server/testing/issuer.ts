// A local stand-in for Microsoft's and Google's OpenID providers: discovery documents, a
// JWKS, and signed id_tokens. Shaped like the real ones (Microsoft's multi-tenant issuer is a
// `{tenantid}` template) so the verifier runs its real checks against it.
import http from "node:http";
import type { AddressInfo } from "node:net";
import { exportJWK, generateKeyPair, SignJWT, type CryptoKey, type JWTPayload } from "jose";

export const TEST_MS_APP = "11111111-2222-3333-4444-555555555555";
export const TEST_GOOGLE_APP = "test-google-app.apps.googleusercontent.com";
export const TENANT = "72f988bf-86f1-41af-91ab-2d7cd011db47";

export type Issuer = Awaited<ReturnType<typeof startIssuer>>;

export async function startIssuer() {
  const { publicKey, privateKey } = await generateKeyPair("RS256");
  const kid = "test-key";
  const jwk = { ...(await exportJWK(publicKey)), kid, alg: "RS256", use: "sig" };

  const server = http.createServer((req, res) => {
    const send = (body: unknown) => {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify(body));
    };
    if (req.url === "/microsoft/v2.0/.well-known/openid-configuration") {
      send({ issuer: `${base}/{tenantid}/v2.0`, jwks_uri: `${base}/keys` });
    } else if (req.url === "/google/.well-known/openid-configuration") {
      send({ issuer: `${base}/google`, jwks_uri: `${base}/keys` });
    } else if (req.url === "/keys") {
      send({ keys: [jwk] });
    } else {
      res.writeHead(404).end();
    }
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;

  async function sign(claims: JWTPayload, opts: { key?: CryptoKey; kid?: string; alg?: string } = {}) {
    const iat = Math.floor(Date.now() / 1000);
    return new SignJWT({ iat, exp: iat + 3600, ...claims })
      .setProtectedHeader({ alg: opts.alg ?? "RS256", kid: opts.kid ?? kid })
      .sign(opts.key ?? privateKey);
  }

  return {
    discoveryUrls: {
      microsoft: `${base}/microsoft/v2.0/.well-known/openid-configuration`,
      google: `${base}/google/.well-known/openid-configuration`,
    },
    sign,
    /** A Microsoft work-account id_token for PeetMail's (test) app. */
    microsoft(claims: JWTPayload & { oid?: string; tid?: string; preferred_username?: string } = {}) {
      const tid = claims.tid ?? TENANT;
      return sign({ iss: `${base}/${tid}/v2.0`, aud: TEST_MS_APP, sub: `pairwise-${claims.oid ?? "o1"}`, tid, oid: "o1", preferred_username: "priya@fabrikam.io", ...claims });
    },
    /** A Google id_token with a verified address. */
    google(claims: JWTPayload & { email?: string; email_verified?: boolean; hd?: string } = {}) {
      return sign({ iss: `${base}/google`, aud: TEST_GOOGLE_APP, sub: "1080001", email: "sam@example.com", email_verified: true, ...claims });
    },
    close: () => new Promise<void>((resolve) => server.close(() => resolve())),
  };
}
