-- PJDesk Cloud accounts, sessions and sync bookkeeping.
--
-- Passwords never reach the service: the client derives an "auth key" from
-- the password (PBKDF2, see src/index.js) and only that is sent; the service
-- stores a salted PBKDF2 hash of it. A separate key, derived from the same
-- password, encrypts the customer's server copy and never leaves their server.

CREATE TABLE accounts (
  id             TEXT PRIMARY KEY,
  email          TEXT NOT NULL UNIQUE,
  auth_hash      TEXT NOT NULL,
  auth_salt      TEXT NOT NULL,
  sync_enabled   INTEGER NOT NULL DEFAULT 0,
  created_at     INTEGER NOT NULL,
  last_login_at  INTEGER
);

CREATE TABLE sessions (
  token_hash    TEXT PRIMARY KEY,
  account_id    TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  device        TEXT NOT NULL DEFAULT '',
  created_at    INTEGER NOT NULL,
  last_used_at  INTEGER NOT NULL
);
CREATE INDEX sessions_by_account ON sessions(account_id);

-- One row per stored server copy (the bytes live in R2).
CREATE TABLE sync_meta (
  account_id  TEXT PRIMARY KEY REFERENCES accounts(id) ON DELETE CASCADE,
  size        INTEGER NOT NULL,
  etag        TEXT NOT NULL,
  updated_at  INTEGER NOT NULL
);

-- Simple rate limiting (failed sign-ins per email, sign-ups per address).
CREATE TABLE rate_events (
  bucket  TEXT NOT NULL,
  at      INTEGER NOT NULL
);
CREATE INDEX rate_events_by_bucket ON rate_events(bucket, at);
