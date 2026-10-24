-- Star, order, and hide on the account's saved computers.
-- A hidden row stays so every signed-in client drops it. Passwords are not stored here.

ALTER TABLE network_computers ADD COLUMN starred INTEGER NOT NULL DEFAULT 0;
ALTER TABLE network_computers ADD COLUMN position INTEGER NOT NULL DEFAULT 0;
ALTER TABLE network_computers ADD COLUMN hidden INTEGER NOT NULL DEFAULT 0;
