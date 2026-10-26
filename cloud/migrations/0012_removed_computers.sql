-- A computer the owner deleted from the account.
-- Presence republish and saved-list uploads do not put it back.
-- Signing in on that computer deletes this row, so it registers again.
-- pjdesk_id on a sign-in lets the delete end that computer's sign-in.

CREATE TABLE network_removed (
  network_id TEXT NOT NULL,
  pjdesk_id TEXT NOT NULL,
  removed_at INTEGER NOT NULL,
  PRIMARY KEY (network_id, pjdesk_id)
);

ALTER TABLE network_devices ADD COLUMN pjdesk_id TEXT NOT NULL DEFAULT '';
