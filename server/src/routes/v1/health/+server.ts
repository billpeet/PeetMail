import { PROTOCOL_VERSION, type Health } from "#lib/protocol.js";
import { route } from "#lib/server/http.js";
import { version } from "../../../../package.json";

/** Lets PeetMail check a server URL before signing in. */
export const GET = route((): Health => ({ ok: true, protocol: PROTOCOL_VERSION, version }));
