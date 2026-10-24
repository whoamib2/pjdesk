-- Saved computer passwords for an account. The value is ciphertext sealed on
-- the device with a key derived from the account password. This service cannot
-- read the passwords. An empty string clears the copy.

CREATE TABLE network_password_vault (
  network_id  TEXT NOT NULL,
  account_id  TEXT NOT NULL,
  ciphertext  TEXT NOT NULL DEFAULT '',
  updated_at  INTEGER NOT NULL,
  PRIMARY KEY (network_id, account_id)
);
