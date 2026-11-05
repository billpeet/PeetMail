import { auth, idParam, route, service } from "#lib/server/http.js";

export const DELETE = route((e) => service().unlinkIdentity(auth(e), idParam(e)));
