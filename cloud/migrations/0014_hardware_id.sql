-- Same physical phone can come back with a new 9-digit id after a reinstall.
-- hardware_id is a hash of the device id, never the raw Android id.

ALTER TABLE network_devices ADD COLUMN hardware_id TEXT NOT NULL DEFAULT '';
ALTER TABLE network_presence ADD COLUMN hardware_id TEXT NOT NULL DEFAULT '';
