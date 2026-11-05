-- Settings sync's view of each synced setting (docs/SYNC.md, src/sync/store.rs): when it was
-- last changed (a hybrid logical clock), the server version it was last in step with, and
-- whether it has a change the server hasn't got. Kept whether or not sync is on, so turning
-- it on later knows which settings were really edited here and which are untouched defaults.
CREATE TABLE sync_items (
  key        TEXT PRIMARY KEY,     -- the settings key, e.g. 'snippet:thanks'
  hlc        TEXT NOT NULL,
  server_seq INTEGER,              -- NULL: never in step with the server
  dirty      INTEGER NOT NULL DEFAULT 0,
  deleted    INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX sync_items_dirty ON sync_items (dirty) WHERE dirty = 1;
