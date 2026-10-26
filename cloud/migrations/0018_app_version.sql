-- PJDesk version last registered by each computer. Empty until a 0.6.44 server publishes it.
ALTER TABLE network_presence ADD COLUMN app_version TEXT NOT NULL DEFAULT '';
