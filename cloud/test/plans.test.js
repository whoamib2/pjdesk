import test from "node:test";
import assert from "node:assert/strict";
import { effectivePlan, isPlan, normalizePlan } from "../src/plans.js";

test("only free and cloud are plans", () => {
  assert.equal(isPlan("free"), true);
  assert.equal(isPlan("cloud"), true);
  assert.equal(isPlan("pro"), false);
  assert.equal(normalizePlan("nope"), "free");
  assert.equal(normalizePlan("cloud"), "cloud");
});

test("admins always have cloud sync", () => {
  const got = effectivePlan({ plan: "free", expiresAt: 1, isAdmin: true, now: 10 });
  assert.deepEqual(got, { plan: "cloud", sync: true, expired: false });
});

test("a cloud grant expires into free", () => {
  const got = effectivePlan({ plan: "cloud", expiresAt: 100, isAdmin: false, now: 100 });
  assert.deepEqual(got, { plan: "free", sync: false, expired: true });
  const later = effectivePlan({ plan: "cloud", expiresAt: 101, isAdmin: false, now: 100 });
  assert.equal(later.sync, true);
  assert.equal(later.expired, false);
});

test("free stays free", () => {
  const got = effectivePlan({ plan: "free", expiresAt: null, isAdmin: false, now: 1 });
  assert.deepEqual(got, { plan: "free", sync: false, expired: false });
});
