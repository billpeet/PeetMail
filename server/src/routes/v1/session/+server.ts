import { parseSessionRequest } from "#lib/protocol.js";
import { body, route, service } from "#lib/server/http.js";

export const POST = route(async (e) => service().signIn(parseSessionRequest(await body(e))));
