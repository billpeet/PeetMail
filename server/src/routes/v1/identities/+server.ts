import { parseLinkRequest } from "#lib/protocol.js";
import { auth, body, route, service } from "#lib/server/http.js";

export const GET = route((e) => service().listIdentities(auth(e)));

export const POST = route(async (e) => {
  const who = auth(e);
  return service().linkIdentity(who, parseLinkRequest(await body(e)));
});
