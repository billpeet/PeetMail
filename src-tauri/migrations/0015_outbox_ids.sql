-- AUTOINCREMENT: outbox ids are never reused. Undo cancels an action's ops by id; with
-- plain rowids a newer op could take the id of one that had already gone out (ids start
-- again at 1 whenever the queue empties), and undo would cancel that op instead.
CREATE TABLE outbox_new (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  account_id  INTEGER NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  remote_id   TEXT,                      -- message the op targets (NULL for new mail)
  op          TEXT NOT NULL,             -- JSON, see outbox.rs `Op`
  status      TEXT NOT NULL DEFAULT 'pending',  -- pending | inflight | failed
  attempts    INTEGER NOT NULL DEFAULT 0,
  last_error  TEXT,
  not_before  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  created_at  TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  series_id   TEXT
);
INSERT INTO outbox_new (id, account_id, remote_id, op, status, attempts, last_error, not_before, created_at, series_id)
  SELECT id, account_id, remote_id, op, status, attempts, last_error, not_before, created_at, series_id FROM outbox;
DROP TABLE outbox;
ALTER TABLE outbox_new RENAME TO outbox;
CREATE INDEX outbox_due ON outbox (status, not_before, id);
CREATE INDEX outbox_remote ON outbox (account_id, remote_id);
