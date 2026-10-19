-- Hostname reported by the remote computer. The name column stays the optional nickname.
ALTER TABLE network_computers ADD COLUMN hostname TEXT NOT NULL DEFAULT '';
