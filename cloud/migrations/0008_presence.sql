-- Computers registered on the owner's server, including ones never saved by hand.
-- console_user is returned only to the network owner.

CREATE TABLE network_presence (
  network_id TEXT NOT NULL,
  pjdesk_id TEXT NOT NULL,
  hostname TEXT NOT NULL DEFAULT '',
  os_family TEXT NOT NULL DEFAULT '',
  os_version TEXT NOT NULL DEFAULT '',
  console_user TEXT NOT NULL DEFAULT '',
  last_seen INTEGER NOT NULL DEFAULT 0,
  online INTEGER NOT NULL DEFAULT 0,
  signed_in INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (network_id, pjdesk_id)
);
