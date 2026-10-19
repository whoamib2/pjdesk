-- Who set or cleared a server address. The admin page shows this list.
-- Registration still works if this table is missing; the log is skipped.

CREATE TABLE network_changes (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  network_id  TEXT NOT NULL,
  at          INTEGER NOT NULL,
  actor       TEXT NOT NULL,
  action      TEXT NOT NULL,
  detail      TEXT NOT NULL DEFAULT ''
);
CREATE INDEX network_changes_by_network ON network_changes(network_id, id);
