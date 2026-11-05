import { parseKeyRequest } from "#lib/protocol.js";
import { auth, body, route, service } from "#lib/server/http.js";

export const PUT = route(async (e) => {
  const who = auth(e);
  service().setKey(who, parseKeyRequest(await body(e)));
});
