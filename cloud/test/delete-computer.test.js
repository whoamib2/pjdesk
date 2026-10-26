import test from "node:test";
import assert from "node:assert/strict";
import { DatabaseSync } from "node:sqlite";
import { forgetComputer, keepListed, ownerMayDelete, pjdeskId } from "../src/network.js";

function envFrom(db) {
  return {
    DB: {
      prepare(sql) {
        return {
          bind(...args) {
            return {
              async run() {
                db.prepare(sql).run(...args);
                return { success: true };
              },
              async all() {
                return { results: db.prepare(sql).all(...args) };
              },
              async first() {
                return db.prepare(sql).get(...args) || null;
              },
            };
          },
        };
      },
    },
  };
}

function seed() {
  const db = new DatabaseSync(":memory:");
  db.exec(`
    CREATE TABLE network_computers (
      network_id TEXT, account_id TEXT, device_id TEXT, name TEXT,
      PRIMARY KEY (network_id, account_id, device_id)
    );
    CREATE TABLE network_presence (
      network_id TEXT, pjdesk_id TEXT, hostname TEXT,
      PRIMARY KEY (network_id, pjdesk_id)
    );
    CREATE TABLE network_devices (
      network_id TEXT, device_id TEXT, pjdesk_id TEXT NOT NULL DEFAULT '',
      PRIMARY KEY (network_id, device_id)
    );
    CREATE TABLE network_removed (
      network_id TEXT, pjdesk_id TEXT, removed_at INTEGER,
      PRIMARY KEY (network_id, pjdesk_id)
    );
  `);
  db.prepare("INSERT INTO network_computers VALUES (?, ?, ?, ?)").run("net_1", "acct", "833201199", "DESKTOP-D7E4AOR");
  db.prepare("INSERT INTO network_computers VALUES (?, ?, ?, ?)").run("net_1", "acct", "692996394", "BENLENOVODESK");
  db.prepare("INSERT INTO network_presence VALUES (?, ?, ?)").run("net_1", "833201199", "DESKTOP-D7E4AOR");
  db.prepare("INSERT INTO network_presence VALUES (?, ?, ?)").run("net_1", "692996394", "BENLENOVODESK");
  db.prepare("INSERT INTO network_devices VALUES (?, ?, ?)").run("net_1", "uuid-desk", "833201199");
  db.prepare("INSERT INTO network_devices VALUES (?, ?, ?)").run("net_1", "uuid-lenovo", "692996394");
  return db;
}

test("a PJDesk id is nine digits", () => {
  assert.equal(pjdeskId("833 201 199"), "833201199");
  assert.equal(pjdeskId("uuid-desk"), "");
  assert.equal(ownerMayDelete("Ben@Spader.us", "ben@spader.us"), true);
  assert.equal(ownerMayDelete("ada@example.com", "ben@spader.us"), false);
});

test("delete removes the saved row, the presence slot, and that sign-in", async () => {
  const db = seed();
  const env = envFrom(db);
  assert.equal(await forgetComputer(env, "net_1", "833 201 199"), "833201199");

  const computers = db.prepare("SELECT device_id FROM network_computers WHERE network_id = ?").all("net_1");
  assert.deepEqual(computers.map((r) => r.device_id), ["692996394"]);
  const presence = db.prepare("SELECT pjdesk_id FROM network_presence WHERE network_id = ?").all("net_1");
  assert.deepEqual(presence.map((r) => r.pjdesk_id), ["692996394"]);
  const devices = db.prepare("SELECT device_id, pjdesk_id FROM network_devices WHERE network_id = ?").all("net_1");
  assert.equal(devices.length, 1);
  assert.equal(devices[0].device_id, "uuid-lenovo");
  assert.equal(devices[0].pjdesk_id, "692996394");
  const removed = db.prepare("SELECT pjdesk_id FROM network_removed").all();
  assert.deepEqual(removed.map((r) => r.pjdesk_id), ["833201199"]);

  const republish = keepListed(
    [
      { id: "833201199", hostname: "DESKTOP-D7E4AOR" },
      { id: "692996394", hostname: "BENLENOVODESK" },
    ],
    ["833201199"],
  );
  assert.deepEqual(republish.map((r) => r.id), ["692996394"]);

  db.prepare("DELETE FROM network_removed WHERE pjdesk_id = ?").run("833201199");
  const again = keepListed([{ id: "833201199", hostname: "DESKTOP-D7E4AOR" }], []);
  assert.equal(again.length, 1);
});

test("a bad id is refused and the other computer stays", async () => {
  const db = seed();
  await assert.rejects(forgetComputer(envFrom(db), "net_1", "not-an-id"), /PJDesk ID/);
  assert.equal(db.prepare("SELECT COUNT(*) AS n FROM network_computers").get().n, 2);
});
