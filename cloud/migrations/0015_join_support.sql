-- 8-digit join codes and quick-support lookups. The digits are never stored.
-- code_hash is SHA-256 of the 8 digits. code_attempts counts guesses by a
-- hash of the caller address, not the address itself.

CREATE TABLE join_codes (
  id          TEXT PRIMARY KEY,
  code_hash   TEXT NOT NULL UNIQUE,
  network_id  TEXT NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
  created_at  INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL,
  used_at     INTEGER,
  revoked_at  INTEGER
);

CREATE TABLE support_offers (
  code_hash   TEXT PRIMARY KEY,
  pjdesk_id   TEXT NOT NULL,
  created_at  INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL
);

CREATE INDEX support_offers_device ON support_offers(pjdesk_id);

CREATE TABLE code_attempts (
  subject      TEXT NOT NULL,
  kind         TEXT NOT NULL,
  failures     INTEGER NOT NULL DEFAULT 0,
  window_start INTEGER NOT NULL,
  locked_until INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (subject, kind)
);
