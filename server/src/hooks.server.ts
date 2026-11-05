import type { ServerInit } from "@sveltejs/kit/hooks";
import { loadConfig } from "#lib/server/config.js";
import { service } from "#lib/server/http.js";

/** Opens the database and checks the configuration at startup, so a mistake shows at once. */
export const init: ServerInit = () => {
  const config = loadConfig();
  const users = service().userCount();
  const rules = config.allowRegister.map((r) => (r.kind === "anyone" ? "anyone" : `${r.kind} ${r.value}`)).join(", ") || "nobody";
  console.log(`PeetMail sync: ${users} sync account(s) in ${config.databasePath}; new accounts allowed for: ${rules}`);
};
