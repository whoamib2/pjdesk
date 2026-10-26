-- Network keys. The blob and its passphrase are never stored. secret_hash is
-- the SHA-256 of the enrollment secret's hex text. uses counts redemptions.

CREATE TABLE network_invites (
  id           TEXT PRIMARY KEY,
  network_id   TEXT NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
  secret_hash  TEXT NOT NULL UNIQUE,
  mode         TEXT NOT NULL,
  bound_email  TEXT NOT NULL DEFAULT '',
  max_uses     INTEGER,
  uses         INTEGER NOT NULL DEFAULT 0,
  expires_at   INTEGER,
  created_at   INTEGER NOT NULL,
  revoked_at   INTEGER
);

CREATE TABLE network_invite_uses (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  invite_id    TEXT NOT NULL REFERENCES network_invites(id) ON DELETE CASCADE,
  device_id    TEXT NOT NULL,
  device_name  TEXT NOT NULL DEFAULT '',
  email        TEXT NOT NULL DEFAULT '',
  used_at      INTEGER NOT NULL
);
