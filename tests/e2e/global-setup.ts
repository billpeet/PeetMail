import { ensureBuilt, sweepStaleDataDirs } from "../harness/app.ts";
import { ensureSyncServerBuilt } from "../harness/sync-server.ts";

// Rebuilds the debug binary only when sources are newer than it, and clears temp data
// dirs that earlier (crashed or interrupted) runs left behind.
export default async function globalSetup() {
  ensureBuilt();
  // Settings sync tests run the real server (settings-sync.spec.ts).
  ensureSyncServerBuilt();
  const swept = await sweepStaleDataDirs();
  if (swept) console.log(`removed ${swept} leftover test data dir(s)`);
}
