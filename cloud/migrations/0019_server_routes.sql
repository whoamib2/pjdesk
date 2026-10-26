-- Both servers' public IPs, LAN IPs, and hostnames. Older clients keep using
-- active_address, active_lan, and active_public_ip. A missing column is ignored
-- until this migration runs.

ALTER TABLE networks ADD COLUMN server_routes TEXT NOT NULL DEFAULT '';
