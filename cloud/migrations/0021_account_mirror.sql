-- Per-network Ed25519 key. PJDesk servers use it to sign access tokens for
-- this network. The service-wide key in network_keys stays on the Worker.
-- Its private key is never copied into the account mirror.
-- The mirror revision is a hash of the snapshot, not a column.

CREATE TABLE network_sign_keys (
  network_id     TEXT PRIMARY KEY REFERENCES networks(id) ON DELETE CASCADE,
  public_key     TEXT NOT NULL,
  private_pkcs8  TEXT NOT NULL,
  created_at     INTEGER NOT NULL
);
