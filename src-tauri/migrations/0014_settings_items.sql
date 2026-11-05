-- Splits, snippets and signatures were each one JSON list in a single setting. They become one
-- setting per entry (docs/SYNC.md, stage 1), so a later sync can merge edits made on two
-- devices entry by entry instead of the last save replacing the whole list.
--
--   split:<id>          {"name", "query", "order"}
--   snippet:<name>      {"name", "body", "order"}       name ASCII-lowercased (names are unique that way)
--   signature:<address> {"html", "replies", "managed"}  address already lowercase
--
-- Values that aren't readable JSON lists are treated as empty, never as an error: a broken
-- setting mustn't stop the app from starting. Every json_each input is checked first, and
-- nested CASEs keep json_type away from invalid JSON (SQLite doesn't promise AND short-circuits).

INSERT INTO settings (key, value)
SELECT 'split:' || lower(hex(randomblob(8))),
       json_object('name', json_extract(j.value, '$.name'), 'query', json_extract(j.value, '$.query'), 'order', j.key + 1)
  FROM json_each(COALESCE((SELECT CASE WHEN json_valid(value) THEN CASE WHEN json_type(value) = 'array' THEN value END END
                             FROM settings WHERE key = 'splits'), '[]')) j
 WHERE j.type = 'object'
   AND json_type(j.value, '$.name') = 'text'
   AND json_type(j.value, '$.query') = 'text'
   -- The pre-built-in-Inbox default started with Important (is:focused), which would claim
   -- nearly all mail ahead of the user's own splits. Dropped once, unless already done.
   AND NOT (json_extract(j.value, '$.name') = 'Important' AND json_extract(j.value, '$.query') = 'is:focused'
            AND NOT EXISTS (SELECT 1 FROM settings WHERE key = 'splits_builtin_inbox'));

-- No list saved yet (a new install), or an unreadable one: the default Other tab. A fixed id,
-- so devices that later sync share one Other tab instead of each adding theirs.
INSERT INTO settings (key, value)
SELECT 'split:other', json_object('name', 'Other', 'query', 'is:other', 'order', 1)
 WHERE NOT EXISTS (SELECT 1 FROM settings
                    WHERE key = 'splits'
                      AND CASE WHEN json_valid(value) THEN json_type(value) = 'array' ELSE 0 END);

INSERT OR REPLACE INTO settings (key, value)
SELECT 'snippet:' || lower(json_extract(j.value, '$.name')),
       json_object('name', json_extract(j.value, '$.name'), 'body', json_extract(j.value, '$.body'), 'order', j.key + 1)
  FROM json_each(COALESCE((SELECT CASE WHEN json_valid(value) THEN CASE WHEN json_type(value) = 'array' THEN value END END
                             FROM settings WHERE key = 'snippets'), '[]')) j
 WHERE j.type = 'object'
   AND json_type(j.value, '$.name') = 'text'
   AND json_type(j.value, '$.body') = 'text'
   AND trim(json_extract(j.value, '$.name')) <> '';

INSERT OR REPLACE INTO settings (key, value)
SELECT 'signature:' || json_extract(j.value, '$.address'),
       json_object('html', COALESCE(json_extract(j.value, '$.html'), ''),
                   'replies', json(CASE WHEN json_extract(j.value, '$.replies') THEN 'true' ELSE 'false' END),
                   'managed', json(CASE WHEN json_extract(j.value, '$.managed') THEN 'true' ELSE 'false' END))
  FROM json_each(COALESCE((SELECT CASE WHEN json_valid(value) THEN CASE WHEN json_type(value) = 'array' THEN value END END
                             FROM settings WHERE key = 'signatures'), '[]')) j
 WHERE j.type = 'object'
   AND json_type(j.value, '$.address') = 'text'
   AND json_extract(j.value, '$.address') <> '';

DELETE FROM settings WHERE key IN ('splits', 'snippets', 'signatures', 'splits_builtin_inbox');
