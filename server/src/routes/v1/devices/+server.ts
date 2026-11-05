import { auth, route, service } from "#lib/server/http.js";

export const GET = route((e) => service().listDevices(auth(e)));
