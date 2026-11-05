// Glue between SvelteKit routes and the service: one service per process, JSON bodies in,
// JSON (or 204) out, SyncErrors as `{error, message}` with their status.
import { json, type RequestEvent, type RequestHandler } from "@sveltejs/kit";
import type { ErrorBody } from "#lib/protocol.js";
import { loadConfig } from "./config.js";
import { openDb } from "./db.js";
import { badRequest, SyncError } from "./errors.js";
import { createVerifier } from "./oidc.js";
import { createService, type Auth, type Service } from "./service.js";

let instance: Service | undefined;

/** The process's service, created on first use from the environment. */
export function service(): Service {
  if (!instance) {
    const config = loadConfig();
    instance = createService(openDb(config.databasePath, config.migrationsDir), createVerifier(config), config);
  }
  return instance;
}

/** Forgets the service, so the next call reads the environment again (tests). */
export function resetService() {
  instance = undefined;
}

export function route(fn: (event: RequestEvent) => unknown): RequestHandler {
  return async (event) => {
    try {
      const result = await fn(event);
      return result === undefined ? new Response(null, { status: 204 }) : json(result);
    } catch (e) {
      if (e instanceof SyncError) return json({ error: e.code, message: e.message } satisfies ErrorBody, { status: e.status });
      console.error(`${event.request.method} ${event.url.pathname} failed:`, e);
      return json({ error: "server_error", message: "Something went wrong on the sync server." } satisfies ErrorBody, { status: 500 });
    }
  };
}

export async function body(event: RequestEvent): Promise<unknown> {
  try {
    return await event.request.json();
  } catch {
    throw badRequest("The request body must be JSON.");
  }
}

export const auth = (event: RequestEvent): Auth => service().authenticate(event.request.headers.get("authorization"));

/** A numeric route parameter. */
export function idParam(event: RequestEvent): number {
  const id = Number(event.params.id);
  if (!Number.isSafeInteger(id) || id <= 0) throw new SyncError(404, "not_found", "Not found.");
  return id;
}
