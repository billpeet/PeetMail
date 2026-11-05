-- The synced list of accounts (docs/SYNC.md, stage 4, src/sync/accounts.rs): one setting per
-- account, written from now on when an account signs in. Accounts already here get theirs
-- now, queued to go up with the zero stamp (they lose to whatever another PC already said
-- about them), so a PC that already syncs shares its accounts too.
INSERT OR IGNORE INTO settings (key, value)
SELECT 'account:' || provider || ':' || lower(email), json_object('app', oauth_app, 'name', display_name)
  FROM accounts;

INSERT OR IGNORE INTO sync_items (key, hlc, dirty)
SELECT 'account:' || provider || ':' || lower(email), '0000000000000:0000:0', 1
  FROM accounts;
