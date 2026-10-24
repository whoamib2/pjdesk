// Temporary codes for 0.6.23.
//
// Join codes are 8 digits, single-use, and last one hour. Support codes are
// also 8 digits and last about 10 minutes. The worker stores SHA-256 hashes,
// never the digits. Guesses are counted per caller and locked after repeated
// failures. Digit bytes use rejection sampling so `byte % 10` is uniform.

export const JOIN_TTL_SEC = 60 * 60;
export const SUPPORT_TTL_SEC = 10 * 60;
export const GUESS_LIMIT = 5;
export const GUESS_WINDOW_SEC = 15 * 60;
export const GUESS_LOCK_SEC = 15 * 60;

/** Bytes above 249 are skipped. 0..249 is 25 complete sets of digits 0-9. */
export function digitsFromBytes(bytes, count = 8) {
  let out = "";
  for (const b of bytes) {
    if (b > 249) continue;
    out += String(b % 10);
    if (out.length >= count) return out.slice(0, count);
  }
  return out;
}

/** Eight digits from the platform CSPRNG (`crypto.getRandomValues`). */
export function randomDigits(count = 8) {
  let out = "";
  while (out.length < count) {
    const need = count - out.length;
    const bytes = crypto.getRandomValues(new Uint8Array(Math.max(need * 2, 16)));
    out += digitsFromBytes(bytes, need);
  }
  return out;
}

export function joinRedeemable({ usedAt, revokedAt, expiresAt, now }) {
  if (usedAt || revokedAt) return false;
  if (!Number.isFinite(expiresAt) || expiresAt <= now) return false;
  return true;
}

/**
 * One step of the guess lockout. `record` counts a failure. `success` clears
 * the counter. A lock that has expired starts a fresh window, so the person
 * gets another set of tries after the wait.
 */
export function nextGuess(row, opts = {}) {
  const limit = opts.limit ?? GUESS_LIMIT;
  const windowSec = opts.windowSec ?? GUESS_WINDOW_SEC;
  const lockSec = opts.lockSec ?? GUESS_LOCK_SEC;
  const now = row.now;
  let failures = Number(row.failures) || 0;
  let windowStart = Number(row.windowStart) || now;
  let lockedUntil = Number(row.lockedUntil) || 0;
  if (row.success) {
    return { failures: 0, windowStart: now, lockedUntil: 0, locked: false, minutes: 0, message: "" };
  }
  if (lockedUntil > now) {
    const minutes = Math.max(1, Math.ceil((lockedUntil - now) / 60));
    const word = minutes === 1 ? "minute" : "minutes";
    return {
      failures,
      windowStart,
      lockedUntil,
      locked: true,
      minutes,
      message: `Too many tries. Try again in ${minutes} ${word}.`,
    };
  }
  if (lockedUntil > 0 && lockedUntil <= now) {
    failures = 0;
    windowStart = now;
    lockedUntil = 0;
  }
  if (now - windowStart >= windowSec) {
    failures = 0;
    windowStart = now;
    lockedUntil = 0;
  }
  if (row.record) {
    failures += 1;
    if (failures >= limit) {
      lockedUntil = now + lockSec;
      const minutes = Math.max(1, Math.ceil(lockSec / 60));
      const word = minutes === 1 ? "minute" : "minutes";
      return {
        failures,
        windowStart,
        lockedUntil,
        locked: true,
        minutes,
        message: `Too many tries. Try again in ${minutes} ${word}.`,
      };
    }
  }
  return { failures, windowStart, lockedUntil, locked: false, minutes: 0, message: "" };
}
