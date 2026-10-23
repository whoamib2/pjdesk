import test from "node:test";
import assert from "node:assert/strict";
import { acceptVault } from "../src/network.js";

test("the password vault accepts only a sealed blob", () => {
  assert.equal(acceptVault(""), null);
  assert.equal(acceptVault(null), null);
  assert.equal(acceptVault("cabin-secret"), "That password copy is not valid.");
  const raw = Buffer.concat([Buffer.from("PJVAULT1"), Buffer.alloc(12), Buffer.alloc(16)]);
  assert.equal(acceptVault(raw.toString("base64")), null);
  assert.match(acceptVault("x".repeat(200001)), /too large/);
});
