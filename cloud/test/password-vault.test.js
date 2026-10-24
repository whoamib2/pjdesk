import test from "node:test";
import assert from "node:assert/strict";
import { acceptLock, acceptVault } from "../src/network.js";

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

test("the password vault accepts only a sealed blob", () => {
  assert.equal(acceptVault(""), null);
  assert.equal(acceptVault(null), null);
  assert.equal(acceptVault("cabin-secret"), "That password copy is not valid.");
  const raw = Buffer.concat([Buffer.from("PJVAULT1"), Buffer.alloc(12), Buffer.alloc(16)]);
  assert.equal(acceptVault(raw.toString("base64")), null);
  assert.match(acceptVault("x".repeat(200001)), /too large/);
});
