-- Support keys extend the 8-digit invite. The digits are never stored.
-- code_hash is SHA-256 of the 8 digits. This is not a server access key
-- and it is not the 10-minute support_offers row.
-- grp is the saved-list group. "Clients" is the support-key group.

ALTER TABLE network_computers ADD COLUMN grp TEXT NOT NULL DEFAULT '';

CREATE TABLE support_keys (
  id              TEXT PRIMARY KEY,
  code_hash       TEXT NOT NULL UNIQUE,
  network_id      TEXT NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
  label           TEXT NOT NULL,
  reusable        INTEGER NOT NULL DEFAULT 0,
  expires_at      INTEGER NOT NULL DEFAULT 0,
  computer_limit  INTEGER NOT NULL DEFAULT 1,
  unattended      INTEGER NOT NULL DEFAULT 0,
  uses            INTEGER NOT NULL DEFAULT 0,
  created_at      INTEGER NOT NULL,
  revoked_at      INTEGER
);

CREATE TABLE support_key_uses (
  key_id      TEXT NOT NULL REFERENCES support_keys(id) ON DELETE CASCADE,
  device_id   TEXT NOT NULL,
  pjdesk_id   TEXT NOT NULL DEFAULT '',
  device_name TEXT NOT NULL DEFAULT '',
  used_at     INTEGER NOT NULL,
  revoked_at  INTEGER,
  PRIMARY KEY (key_id, device_id)
);

CREATE TABLE support_connections (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  key_id    TEXT NOT NULL,
  pjdesk_id TEXT NOT NULL DEFAULT '',
  from_name TEXT NOT NULL DEFAULT '',
  at        INTEGER NOT NULL
);

CREATE INDEX support_connections_key ON support_connections(key_id, at);
