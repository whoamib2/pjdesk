-- SHA-256 of the per-install key. Empty until a 0.6.47 server publishes it.
-- Two computers that present different keys for one ID stay two rows.
ALTER TABLE network_presence ADD COLUMN install_key TEXT NOT NULL DEFAULT '';
