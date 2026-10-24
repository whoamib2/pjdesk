-- Private networks. A PJDesk account owns one network: the server addresses
-- that account's clients may use, the other accounts allowed to use it, and
-- the devices that have signed in. Passwords for remote computers are not
-- stored here.

CREATE TABLE networks (
  id                   TEXT PRIMARY KEY,
  owner_id             TEXT NOT NULL REFERENCES accounts(id) ON DELETE CASCADE,
  name                 TEXT NOT NULL DEFAULT '',
  active_address       TEXT NOT NULL DEFAULT '',
  active_fingerprint   TEXT NOT NULL DEFAULT '',
  standby_address      TEXT NOT NULL DEFAULT '',
  standby_fingerprint  TEXT NOT NULL DEFAULT '',
  require_accounts     INTEGER NOT NULL DEFAULT 0,
  created_at           INTEGER NOT NULL
);
CREATE UNIQUE INDEX networks_by_owner ON networks(owner_id);

CREATE TABLE network_members (
  network_id  TEXT NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
  email       TEXT NOT NULL,
  account_id  TEXT,
  created_at  INTEGER NOT NULL,
  revoked_at  INTEGER,
  PRIMARY KEY (network_id, email)
);

CREATE TABLE network_devices (
  network_id    TEXT NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
  device_id     TEXT NOT NULL,
  account_id    TEXT,
  email         TEXT NOT NULL DEFAULT '',
  name          TEXT NOT NULL DEFAULT '',
  refresh_hash  TEXT NOT NULL DEFAULT '',
  enrolled_at   INTEGER NOT NULL,
  revoked_at    INTEGER,
  PRIMARY KEY (network_id, device_id)
);

-- Saved computers (name, id, OS). No password column.
CREATE TABLE network_computers (
  network_id  TEXT NOT NULL,
  account_id  TEXT NOT NULL,
  device_id   TEXT NOT NULL,
  name        TEXT NOT NULL DEFAULT '',
  os_family   TEXT NOT NULL DEFAULT '',
  os_version  TEXT NOT NULL DEFAULT '',
  updated_at  INTEGER NOT NULL,
  PRIMARY KEY (network_id, account_id, device_id)
);

-- One Ed25519 key for the whole service. The private key never leaves the Worker.
CREATE TABLE network_keys (
  id             INTEGER PRIMARY KEY CHECK (id = 1),
  public_key     TEXT NOT NULL,
  private_pkcs8  TEXT NOT NULL
);

CREATE TABLE enroll_codes (
  code_hash   TEXT PRIMARY KEY,
  network_id  TEXT NOT NULL REFERENCES networks(id) ON DELETE CASCADE,
  created_at  INTEGER NOT NULL,
  expires_at  INTEGER NOT NULL,
  used_at     INTEGER
);
