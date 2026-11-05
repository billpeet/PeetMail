-- Mail found by searching the server, and the rest of each conversation opened from the
-- results. Rows stay out of the cache-window prune while the search is open. When it
-- closes, mail that wasn't cached before the search (`added`) is dropped again.
-- `hit`: the server matched this message, so it shows as a result.
CREATE TABLE search_cache (
  message_id INTEGER PRIMARY KEY REFERENCES messages(id) ON DELETE CASCADE,
  hit        INTEGER NOT NULL DEFAULT 1,
  added      INTEGER NOT NULL DEFAULT 0
);
