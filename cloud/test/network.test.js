import test from "node:test";
import assert from "node:assert/strict";
import { canonical, decide, inviteError, normalizeFingerprint } from "../src/network.js";

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
