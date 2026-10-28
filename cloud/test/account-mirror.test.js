import test from "node:test";
import assert from "node:assert/strict";
import { applyChange, assembleMirror, decidePut, revisionOf } from "../src/account-mirror.js";

const owner = {
  id: "acct",
  email: "ben@spader.us",
  auth_hash: "aa",
  auth_salt: "bb",
  plan: "cloud",
  plan_expires_at: 99,
  sync_enabled: 1,
};

function copy() {
  return assembleMirror({
    owner,
    network: { id: "net", active_address: "203.0.113.10:21200", active_fingerprint: "ab" },
    cloudPublic: "cloud-public",
    signPublic: "net-public",
    signPkcs8: "net-private",
  });
}

test("the mirror carries verifiers and the per-network key, not the service private key", () => {
  const mirror = copy();
  assert.equal(mirror.auth_hash, "aa");
  assert.equal(mirror.cloud_public, "cloud-public");
  assert.equal(mirror.sign_pkcs8, "net-private");
  assert.equal(mirror.plan, "cloud");
  assert.equal("private_pkcs8" in mirror, false);
  assert.equal(JSON.stringify(mirror).includes("password"), false);
});

test("a saved-list edit does not replace the website password or plan", () => {
  const mirror = copy();
  applyChange(mirror, {
    op: "computer",
    computer: { id: "123456789", account_id: "acct", name: "Surface", updated_at: 10 },
  });
  assert.equal(mirror.auth_hash, "aa");
  assert.equal(mirror.auth_salt, "bb");
  assert.equal(mirror.plan, "cloud");
  assert.equal(mirror.plan_expires_at, 99);
  assert.equal(mirror.computers[0].name, "Surface");
  applyChange(mirror, {
    op: "computer",
    computer: { id: "123456789", account_id: "acct", name: "Older", updated_at: 5 },
  });
  assert.equal(mirror.computers[0].name, "Surface");
});

test("a newer removal sticks, and a revocation newer than the enroll is kept", () => {
  const mirror = copy();
  applyChange(mirror, { op: "computer", computer: { id: "123456789", account_id: "acct", updated_at: 10 } });
  applyChange(mirror, { op: "remove", id: "123456789", at: 20 });
  assert.equal(mirror.computers.length, 0);
  applyChange(mirror, { op: "computer", computer: { id: "123456789", account_id: "acct", updated_at: 15 } });
  assert.equal(mirror.computers.length, 0);
  applyChange(mirror, {
    op: "device",
    device: { device_id: "phone", refresh_hash: "hh", enrolled_at: 5 },
  });
  applyChange(mirror, { op: "revoke_device", device_id: "phone", at: 30 });
  applyChange(mirror, {
    op: "device",
    device: { device_id: "phone", refresh_hash: "new", enrolled_at: 10 },
  });
  assert.equal(mirror.devices[0].revoked_at, 30);
  assert.equal(mirror.devices[0].refresh_hash, "");
});

test("a sign key change replaces the exposed key", () => {
  const mirror = copy();
  applyChange(mirror, { op: "sign_key", public_key: "ab".repeat(32), pkcs8: "cd".repeat(48) });
  assert.equal(mirror.sign_public, "ab".repeat(32));
  assert.equal(mirror.sign_pkcs8, "cd".repeat(48));
  assert.equal(mirror.sign_pkcs8.includes("net-private"), false);
});

test("an empty or stale base revision conflicts, and a matching one applies", () => {
  assert.equal(decidePut("abc", ""), "conflict");
  assert.equal(decidePut("abc", "zzz"), "conflict");
  assert.equal(decidePut("abc", "abc"), "apply");
});

test("the revision changes when the saved list changes and ignores key order", async () => {
  const first = copy();
  first.revision = await revisionOf(first);
  const again = copy();
  again.revision = await revisionOf(again);
  assert.equal(first.revision, again.revision);
  applyChange(again, { op: "computer", computer: { id: "123456789", account_id: "acct", name: "Desk", updated_at: 1 } });
  again.revision = await revisionOf(again);
  assert.notEqual(first.revision, again.revision);
});
