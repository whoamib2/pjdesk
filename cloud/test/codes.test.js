import test from "node:test";
import assert from "node:assert/strict";
import { digitsFromBytes, joinRedeemable, nextGuess, randomDigits, GUESS_LIMIT, GUESS_LOCK_SEC } from "../src/codes.js";

test("digit bytes skip the biased tail and stay decimal", () => {
  assert.equal(digitsFromBytes([0, 9, 10, 249, 250, 255, 3], 8), "09093");
  assert.equal(digitsFromBytes([250, 251, 255], 8), "");
  assert.equal(digitsFromBytes([1, 2, 3, 4, 5, 6, 7, 8, 9], 8), "12345678");
  const code = randomDigits(8);
  assert.equal(code.length, 8);
  assert.match(code, /^[0-9]{8}$/);
});

test("a join code is single-use, revocable, and expired at the deadline", () => {
  assert.equal(joinRedeemable({ expiresAt: 100, now: 99 }), true);
  assert.equal(joinRedeemable({ expiresAt: 100, now: 100 }), false);
  assert.equal(joinRedeemable({ expiresAt: 100, usedAt: 50, now: 60 }), false);
  assert.equal(joinRedeemable({ expiresAt: 100, revokedAt: 40, now: 60 }), false);
});

test("five wrong guesses lock the caller, then a later success clears it", () => {
  let row = { failures: 0, windowStart: 1_000, lockedUntil: 0, now: 1_000 };
  for (let i = 1; i < GUESS_LIMIT; i++) {
    row = { ...nextGuess({ ...row, record: true }), now: 1_000 };
    assert.equal(row.locked, false, `failure ${i} should still be open`);
  }
  const locked = nextGuess({ ...row, record: true, now: 1_000 });
  assert.equal(locked.locked, true);
  assert.equal(locked.lockedUntil, 1_000 + GUESS_LOCK_SEC);
  assert.match(locked.message, /Too many tries/);
  const still = nextGuess({ ...locked, now: 1_000 + 10, record: true });
  assert.equal(still.locked, true);
  assert.equal(still.failures, locked.failures);
  const after = nextGuess({ ...locked, now: locked.lockedUntil, record: false });
  assert.equal(after.locked, false);
  assert.equal(after.failures, 0);
  const cleared = nextGuess({ ...locked, now: 1_000, success: true });
  assert.equal(cleared.locked, false);
  assert.equal(cleared.failures, 0);
});
