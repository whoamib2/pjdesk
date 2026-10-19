-- Installer or Settings name stored with a saved computer.
-- Empty means show the hostname. An upload that omits the name keeps the stored one.

ALTER TABLE network_computers ADD COLUMN display_name TEXT NOT NULL DEFAULT '';
