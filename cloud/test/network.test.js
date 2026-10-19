import test from "node:test";
import assert from "node:assert/strict";
import { canonical, decide, normalizeFingerprint } from "../src/network.js";

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
