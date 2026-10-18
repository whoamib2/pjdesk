-- Plans are Free or Cloud Sync. No prices: the site does not sell tiers.
-- Existing accounts that already had Cloud Sync on become Cloud.

ALTER TABLE accounts ADD COLUMN plan TEXT NOT NULL DEFAULT 'free';
ALTER TABLE accounts ADD COLUMN plan_expires_at INTEGER;
ALTER TABLE accounts ADD COLUMN plan_note TEXT NOT NULL DEFAULT '';

UPDATE accounts SET plan = 'cloud' WHERE sync_enabled = 1;

CREATE TABLE plan_audit (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  at INTEGER NOT NULL,
  actor TEXT NOT NULL,
  email TEXT NOT NULL,
  old_plan TEXT NOT NULL,
  new_plan TEXT NOT NULL,
  expires_at INTEGER,
  note TEXT NOT NULL DEFAULT ''
);

-- One row per PJDesk Server that has reported health for an account.
CREATE TABLE server_health (
  account_id TEXT NOT NULL,
  instance TEXT NOT NULL,
  name TEXT NOT NULL DEFAULT '',
  version TEXT NOT NULL DEFAULT '',
  role TEXT NOT NULL DEFAULT '',
  epoch INTEGER NOT NULL DEFAULT 0,
  healthy INTEGER NOT NULL DEFAULT 0,
  reported_at INTEGER NOT NULL,
  PRIMARY KEY (account_id, instance)
);
