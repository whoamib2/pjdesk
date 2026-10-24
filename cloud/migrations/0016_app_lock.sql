-- App-lock PIN for an account. The PIN is never stored. The value is
-- ciphertext sealed on the device with the same key as the password vault
-- (PJLOCK01). This service cannot read it. An empty string clears the PIN.

CREATE TABLE network_app_lock (
  network_id  TEXT NOT NULL,
  account_id  TEXT NOT NULL,
  ciphertext  TEXT NOT NULL DEFAULT '',
  updated_at  INTEGER NOT NULL,
  PRIMARY KEY (network_id, account_id)
);
