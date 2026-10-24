import test from "node:test";
import assert from "node:assert/strict";
import { acceptLock, acceptVault, appLockAuthMode } from "../src/network.js";

test("the app lock accepts only a sealed blob", () => {
  assert.equal(acceptLock(""), null);
  assert.equal(acceptLock(null), null);
  assert.equal(acceptLock("1234"), "That app lock is not valid.");
  const raw = Buffer.concat([Buffer.from("PJLOCK01"), Buffer.alloc(12), Buffer.alloc(16)]);
  assert.equal(acceptLock(raw.toString("base64")), null);
  const vault = Buffer.concat([Buffer.from("PJVAULT1"), Buffer.alloc(12), Buffer.alloc(16)]);
  assert.equal(acceptLock(vault.toString("base64")), "That app lock is not valid.");
  assert.match(acceptLock("x".repeat(8001)), /too large/);
});

test("the server window uses its pjdesk.com sign-in for the same PIN", () => {
  assert.equal(appLockAuthMode("Bearer abcdefghij1234567890", {}), "bearer");
  assert.equal(appLockAuthMode("Bearer abcdefghij1234567890", { refresh: "aa" }), "device");
  assert.equal(appLockAuthMode("", { refresh: "aa", device_id: "dev" }), "device");
  assert.equal(appLockAuthMode("Basic abc", {}), "device");
});

test("the password vault accepts only a sealed blob", () => {
  assert.equal(acceptVault(""), null);
  assert.equal(acceptVault(null), null);
  assert.equal(acceptVault("cabin-secret"), "That password copy is not valid.");
  const raw = Buffer.concat([Buffer.from("PJVAULT1"), Buffer.alloc(12), Buffer.alloc(16)]);
  assert.equal(acceptVault(raw.toString("base64")), null);
  assert.match(acceptVault("x".repeat(200001)), /too large/);
});
