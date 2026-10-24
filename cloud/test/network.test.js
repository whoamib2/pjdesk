import test from "node:test";
import assert from "node:assert/strict";
import { applyPresence, canonical, decide, inviteError, isPrivateAddress, normalizeFingerprint, planRegistration, presenceView, requireAccountsOn, savedShape } from "../src/network.js";

test("canonical matches the shared fixture", () => {
  const text = canonical({
    email: "Ben@Spader.us",
    networkId: "net_1",
    fingerprints: ["AA:BB:CC:DD", "aabbccdd", "00112233"],
    deviceId: "device-1",
    exp: 2000000000,
  });
  assert.equal(text, "PJN1\nben@spader.us\nnet_1\n00112233,aabbccdd\ndevice-1\n2000000000\n");
  assert.equal(normalizeFingerprint("sha256:AB-CD"), "abcd");
});

test("a fresh network leaves require signed-in off", () => {
  assert.equal(requireAccountsOn(0), false);
  assert.equal(requireAccountsOn(false), false);
  assert.equal(requireAccountsOn(null), false);
  assert.equal(requireAccountsOn(undefined), false);
  assert.equal(requireAccountsOn("0"), false);
  assert.equal(requireAccountsOn(""), false);
  assert.equal(requireAccountsOn(1), true);
  assert.equal(requireAccountsOn(true), true);
});

test("decide matches grace, expiry, the wrong server, and revoke", () => {
  assert.equal(decide({ requireAccounts: false, verified: false }), "open");
  assert.equal(decide({ requireAccounts: true, verified: false }), "deny");
  assert.equal(decide({ requireAccounts: true, verified: true, expired: true }), "deny");
  assert.equal(decide({ requireAccounts: false, verified: true, expired: true }), "open");
  assert.equal(decide({ requireAccounts: true, verified: true, wrongServer: true }), "deny");
  assert.equal(
    decide({ requireAccounts: false, verified: true, revoked: true, email: "ben@spader.us" }),
    "deny",
  );
  assert.equal(
    decide({
      requireAccounts: false,
      verified: true,
      allowed: ["ada@example.com"],
      email: "ben@spader.us",
    }),
    "deny",
  );
  assert.equal(
    decide({
      requireAccounts: true,
      verified: true,
      allowed: ["ben@spader.us"],
      email: "ben@spader.us",
    }),
    "signed",
  );
});

test("a network key is refused when it is expired, revoked, or used up", () => {
  assert.equal(inviteError({ revoked: true, uses: 0, now: 10 }), "This network key was revoked.");
  assert.equal(inviteError({ expiresAt: 10, uses: 0, now: 10 }), "This network key has expired.");
  assert.equal(inviteError({ expiresAt: 11, uses: 0, now: 10 }), null);
  assert.equal(
    inviteError({ uses: 2, maxUses: 2, now: 10 }),
    "This network key has already been used the maximum number of times.",
  );
  assert.equal(inviteError({ uses: 1, maxUses: 2, now: 10 }), null);
  assert.equal(
    inviteError({ uses: 0, now: 10, boundEmail: "Ben@Spader.us" }),
    "This network key is for a specific account. Sign in as that account.",
  );
  assert.equal(inviteError({ uses: 0, now: 10, boundEmail: "ben@spader.us", callerEmail: "ben@spader.us" }), null);
});

test("an owner server fills an empty slot and does not overwrite a different key", () => {
  const empty = planRegistration({
    owner: true,
    role: "active",
    address: "pjdesk.spader.us:21200",
    fingerprint: "ab-cd",
    confirm: false,
    replace: false,
    activeAddress: "",
    activeFingerprint: "",
    standbyAddress: "",
    standbyFingerprint: "",
  });
  assert.equal(empty.status, 200);
  assert.equal(empty.next.activeAddress, "pjdesk.spader.us:21200");
  assert.equal(empty.next.activeFingerprint, "abcd");

  const sharedKey = planRegistration({
    owner: true,
    role: "standby",
    address: "68.234.75.68:21200",
    fingerprint: "abcd",
    activeAddress: "pjdesk.spader.us:21200",
    activeFingerprint: "abcd",
    standbyAddress: "",
    standbyFingerprint: "",
  });
  assert.equal(sharedKey.status, 200);
  assert.equal(sharedKey.next.activeAddress, "pjdesk.spader.us:21200");
  assert.equal(sharedKey.next.standbyAddress, "68.234.75.68:21200");

  const flap = planRegistration({
    owner: true,
    role: "standby",
    address: "204.248.122.44:21200",
    fingerprint: "abcd",
    instance: "desk",
    activeAddress: "",
    activeFingerprint: "",
    standbyAddress: "68.234.75.68:21200",
    standbyFingerprint: "abcd",
    standbyInstance: "cabin",
  });
  assert.equal(flap.status, 409);
  assert.equal(flap.changed, false);
  assert.equal(flap.next.standbyAddress, "68.234.75.68:21200");

  const owned = planRegistration({
    owner: true,
    role: "active",
    address: "pjdesk.spader.us:21200",
    fingerprint: "abcd",
    instance: "desk",
    lan: ["192.168.1.20:21200"],
    publicIp: "204.248.122.44",
    activeAddress: "204.248.122.44:21200",
    activeFingerprint: "abcd",
    activeInstance: "desk",
    standbyAddress: "68.234.75.68:21200",
    standbyFingerprint: "abcd",
    standbyInstance: "cabin",
  });
  assert.equal(owned.status, 200);
  assert.equal(owned.changed, true);
  assert.equal(owned.next.activeAddress, "pjdesk.spader.us:21200");
  assert.equal(owned.next.activePublicIp, "204.248.122.44");
  assert.equal(owned.next.activeLan, "192.168.1.20:21200");
  assert.equal(owned.next.standbyAddress, "68.234.75.68:21200");

  const quiet = planRegistration({
    owner: true,
    role: "active",
    address: "pjdesk.spader.us:21200",
    fingerprint: "abcd",
    instance: "desk",
    publicIp: "204.248.122.44",
    activeAddress: "pjdesk.spader.us:21200",
    activeFingerprint: "abcd",
    activeInstance: "desk",
    activePublicIp: "204.248.122.44",
  });
  assert.equal(quiet.changed, false);

  const blocked = planRegistration({
    owner: true,
    role: "active",
    address: "other.example:21200",
    fingerprint: "ffff",
    activeAddress: "pjdesk.spader.us:21200",
    activeFingerprint: "abcd",
    standbyAddress: "",
    standbyFingerprint: "",
  });
  assert.equal(blocked.status, 409);
  assert.equal(blocked.needsConfirm, true);
  assert.equal(blocked.next.activeAddress, "pjdesk.spader.us:21200");

  const replaced = planRegistration({
    owner: true,
    role: "active",
    address: "other.example:21200",
    fingerprint: "ffff",
    replace: true,
    activeAddress: "pjdesk.spader.us:21200",
    activeFingerprint: "abcd",
    standbyAddress: "",
    standbyFingerprint: "",
  });
  assert.equal(replaced.status, 200);
  assert.equal(replaced.next.activeFingerprint, "ffff");

  const stranger = planRegistration({
    owner: false,
    role: "active",
    address: "other.example:21200",
    fingerprint: "ffff",
    replace: true,
    confirm: true,
    activeAddress: "pjdesk.spader.us:21200",
    activeFingerprint: "abcd",
  });
  assert.equal(stranger.status, 403);

  assert.equal(isPrivateAddress("192.168.1.20:21200"), true);
  assert.equal(isPrivateAddress("pjdesk.spader.us:21200"), false);
  const lan = planRegistration({
    owner: true,
    role: "active",
    address: "192.168.1.20:21200",
    fingerprint: "abcd",
    confirm: false,
    activeAddress: "",
    activeFingerprint: "",
  });
  assert.equal(lan.status, 400);
  const confirmedLan = planRegistration({
    owner: true,
    role: "active",
    address: "192.168.1.20:21200",
    fingerprint: "abcd",
    confirm: true,
    activeAddress: "",
    activeFingerprint: "",
  });
  assert.equal(confirmedLan.status, 200);
});

test("an empty host report does not erase the name or the user", () => {
  const kept = applyPresence(
    { hostname: "BENLENOVODESK", os_family: "windows", os_version: "11", console_user: "SPADER\\Ben", last_seen: 10 },
    { hostname: "", os_family: "", os_version: "", console_user: "", last_seen: 4, online: false },
  );
  assert.equal(kept.hostname, "BENLENOVODESK");
  assert.equal(kept.console_user, "SPADER\\Ben");
  assert.equal(kept.os_family, "windows");
  assert.equal(kept.last_seen, 10);
  const newer = applyPresence(kept, { hostname: "BEN-SURFACE-9", console_user: "Ben", last_seen: 20, online: true, os_family: "windows", os_version: "11" });
  assert.equal(newer.hostname, "BEN-SURFACE-9");
  assert.equal(newer.console_user, "Ben");
  assert.equal(newer.online, 1);
  assert.equal(presenceView(newer, "owner").console_user, "Ben");
  assert.equal(presenceView(newer, "member").console_user, "");
});

test("a saved computer keeps the nickname, star, and order, and never a password", () => {
  const row = savedShape({
    id: "902 488 771",
    name: "Cabin",
    hostname: "CABINDESK",
    os_family: "windows",
    os_version: "11",
    starred: true,
    position: 20,
    password: "correct horse",
    display_name: "Cabin desk",
  });
  assert.equal(row.id, "902488771");
  assert.equal(row.name, "Cabin");
  assert.equal(row.display_name, "Cabin desk");
  assert.equal(row.starred, true);
  assert.equal(row.position, 20);
  assert.equal(row.hidden, false);
  assert.equal(JSON.stringify(row).includes("password"), false);
  assert.equal(JSON.stringify(row).includes("correct horse"), false);
  const hidden = savedShape({ id: "111111111", name: "Gone", hidden: true });
  assert.equal(hidden.hidden, true);
  assert.equal(hidden.name, "");
});

test("a saved computer has no password field in the canonical record", () => {
  const text = canonical({
    email: "ben@spader.us",
    networkId: "net_1",
    fingerprints: [],
    deviceId: "device-1",
    exp: 1,
  });
  assert.equal(text.includes("password"), false);
});

test("an empty display name clears, and a missing one does not", () => {
  const named = applyPresence({ hostname: "CABINDESK", display_name: "Cabin" }, { display_name: "" });
  assert.equal(named.display_name, "");
  assert.equal(named.hostname, "CABINDESK");
  const kept = applyPresence({ display_name: "Cabin", hostname: "CABINDESK" }, { hostname: "" });
  assert.equal(kept.display_name, "Cabin");
  const view = presenceView({ id: "1", hostname: "CABINDESK", display_name: "Cabin", console_user: "Ben" }, "member");
  assert.equal(view.display_name, "Cabin");
  assert.equal(view.console_user, "");
});
