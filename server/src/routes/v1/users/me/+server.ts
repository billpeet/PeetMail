import { auth, route, service } from "#lib/server/http.js";

export const DELETE = route((e) => service().deleteUser(auth(e)));
