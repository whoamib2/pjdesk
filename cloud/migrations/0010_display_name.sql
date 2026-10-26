-- Friendly name a computer chose for itself. Empty means show the hostname.
-- A report with an empty name clears the stored one.

ALTER TABLE network_presence ADD COLUMN display_name TEXT NOT NULL DEFAULT '';
