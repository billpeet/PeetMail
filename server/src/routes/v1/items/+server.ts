import { LIMITS, parseCount, parsePushRequest } from "#lib/protocol.js";
import { auth, body, route, service } from "#lib/server/http.js";

export const GET = route((e) => {
  const who = auth(e);
  const since = parseCount(e.url.searchParams.get("since"), 0, Number.MAX_SAFE_INTEGER, "since");
  const limit = Math.max(1, parseCount(e.url.searchParams.get("limit"), LIMITS.pullItems, LIMITS.pullItems, "limit"));
  return service().pull(who, since, limit);
});

export const PUT = route(async (e) => {
  const who = auth(e);
  return service().push(who, parsePushRequest(await body(e)).items);
});
