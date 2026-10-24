-- LAN addresses and the observed public IP for each server slot.
-- The address column stays a hostname when the operator set one.
-- require_accounts is not changed here; grace stays off unless the operator turns it on.

ALTER TABLE networks ADD COLUMN active_lan TEXT NOT NULL DEFAULT '';
ALTER TABLE networks ADD COLUMN standby_lan TEXT NOT NULL DEFAULT '';
ALTER TABLE networks ADD COLUMN active_public_ip TEXT NOT NULL DEFAULT '';
ALTER TABLE networks ADD COLUMN standby_public_ip TEXT NOT NULL DEFAULT '';
ALTER TABLE networks ADD COLUMN active_instance TEXT NOT NULL DEFAULT '';
ALTER TABLE networks ADD COLUMN standby_instance TEXT NOT NULL DEFAULT '';
ALTER TABLE networks ADD COLUMN active_ip_checked_at INTEGER NOT NULL DEFAULT 0;
ALTER TABLE networks ADD COLUMN standby_ip_checked_at INTEGER NOT NULL DEFAULT 0;
