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

/** Seconds a support key may live. 0 means it does not expire. */
export const SUPPORT_KEY_EXPIRY = [3600, 86400, 7 * 86400, 30 * 86400, 0];

/** One of the allowed expiries, or null when the choice is not one of them. */
export function supportKeyExpiry(seconds) {
  const n = Number(seconds);
  return SUPPORT_KEY_EXPIRY.includes(n) ? n : null;
}

/**
 * Whether this support key can enroll a computer.
 * `uses` counts every new computer, including ones later removed.
 * `computers` counts computers still enrolled. The same computer does not
 * take another slot. Single-use stops after the first new computer.
 * Reusable stops when the live count reaches the limit.
 */
export function supportKeyStatus({ revokedAt, expiresAt, reusable, uses, computers, limit, now, sameDevice }) {
  if (revokedAt) return { ok: false, reason: "revoked" };
  if (Number(expiresAt) > 0 && Number(expiresAt) <= now) return { ok: false, reason: "expired" };
  if (sameDevice) return { ok: true, reason: "" };
  const cap = Math.max(1, Number(limit) || 1);
  const enrolled = Number(computers) || 0;
  const ever = Number(uses) || 0;
  if (!reusable && ever > 0) return { ok: false, reason: "used" };
  if (enrolled >= cap) return { ok: false, reason: "limit" };
  return { ok: true, reason: "" };
}

/** Network name when it is set, otherwise the account email. */
export function supportOwnerName(networkName, email) {
  const name = String(networkName || "")
    .replace(/[\u0000-\u001f\u007f]/g, "")
    .trim()
    .slice(0, 80);
  return name || String(email || "").trim();
}

/** The line the supported computer shows. */
export function supportBanner(owner) {
  const who = String(owner || "").trim() || "Support";
  return `${who} (support) has access`;
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
