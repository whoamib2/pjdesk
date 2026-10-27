import test from "node:test";
import assert from "node:assert/strict";
import {
  LOGIN_FAILS,
  LOGIN_IP,
  loginRetryAfter,
  retryAfterSeconds,
  signInLimitMessage,
} from "../src/login-limit.js";

test("the sign-in limit is 10 failures per email and 30 per IP, each for 15 minutes", () => {
  assert.equal(LOGIN_FAILS.limit, 10);
  assert.equal(LOGIN_FAILS.window, 15 * 60);
  assert.equal(LOGIN_IP.limit, 30);
  assert.equal(LOGIN_IP.window, 15 * 60);
});

test("ten failures block until the oldest one leaves the window", () => {
  const now = 1_000_000;
  const window = LOGIN_FAILS.window;
  const events = Array.from({ length: 10 }, (_, i) => now - window + 60 + i);
  assert.equal(loginRetryAfter(events, [], now), 60);
  assert.equal(signInLimitMessage(60), "Too many sign-in attempts. Try again in 1 minute.");
  assert.equal(signInLimitMessage(61), "Too many sign-in attempts. Try again in 2 minutes.");
  assert.equal(signInLimitMessage(1), "Too many sign-in attempts. Try again in 1 minute.");
  assert.equal(signInLimitMessage(15 * 60), "Too many sign-in attempts. Try again in 15 minutes.");
});

test("nine failures do not block, and an event on the window edge does not count", () => {
  const now = 5_000;
  const nine = Array.from({ length: 9 }, (_, i) => now - i);
  assert.equal(loginRetryAfter(nine, [], now), 0);
  const expired = Array.from({ length: 10 }, () => now - LOGIN_FAILS.window);
  assert.equal(loginRetryAfter(expired, [], now), 0);
  assert.equal(retryAfterSeconds(nine, now, LOGIN_FAILS), 0);
});

test("a full IP bucket blocks even when the email bucket is empty, and the longer wait wins", () => {
  const now = 8_000;
  const ip = Array.from({ length: 30 }, (_, i) => now - 100 - i);
  const ipWait = LOGIN_IP.window - 129;
  assert.equal(loginRetryAfter([], ip, now), ipWait);
  const email = Array.from({ length: 10 }, () => now - 10);
  const emailWait = LOGIN_FAILS.window - 10;
  assert.equal(loginRetryAfter(email, ip, now), Math.max(emailWait, ipWait));
});
