// Website download password, the same idea as ngnvr.com: checked only in this
// Worker, never hard-coded. Ben sets the Worker secret DOWNLOAD_PASSWORD
// himself (`npx wrangler secret put DOWNLOAD_PASSWORD`). A right guess gives
// the browser a 30-day signed cookie; /download/client|server|sums then
// redirect to the files. Auto-update keeps using GET /api/release, which is
// not gated — existing installs and the Android app must keep working.
//
// This only locks the website buttons and /download/* redirects. The GitHub
// release assets stay public (unlike NGNVR, which serves installers from R2
// behind the same gate).

const encoder = new TextEncoder();

export const COOKIE_BASE = "pjdesk_dl";
export const UNLOCK_DAYS = 30;
export const UNLOCK_FAILS = { limit: 10, window: 15 * 60 };

export class DownloadError extends Error {
  constructor(status, message, code, retryAfter) {
    super(message);
    this.status = status;
    this.code = code;
    this.retryAfter = retryAfter;
  }
}

export const now = () => Math.floor(Date.now() / 1000);

export function timingSafeEqual(a, b) {
  const ea = encoder.encode(String(a));
  const eb = encoder.encode(String(b));
  if (ea.length !== eb.length) return false;
  let d = 0;
  for (let i = 0; i < ea.length; i++) d |= ea[i] ^ eb[i];
  return d === 0;
}

function toHex(buf) {
  return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

async function sha256Hex(text) {
  return toHex(await crypto.subtle.digest("SHA-256", encoder.encode(text)));
}

/** Compare a guess to the secret without leaking length or an early mismatch. */
export async function passwordsMatch(guess, secret) {
  const a = await sha256Hex(String(guess ?? "").normalize("NFKC"));
  const b = await sha256Hex(String(secret ?? "").normalize("NFKC"));
  return timingSafeEqual(a, b);
}

async function hmacHex(password, data) {
  const key = await crypto.subtle.importKey(
    "raw",
    encoder.encode(password),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  return toHex(await crypto.subtle.sign("HMAC", key, encoder.encode(data)));
}

/** HMAC over "download:<expiry>:<sha256(password)>", so changing the secret ends every cookie. */
export async function signCookie(password, exp) {
  const digest = await sha256Hex(String(password).normalize("NFKC"));
  return hmacHex(password, `download:${exp}:${digest}`);
}

export async function makeCookieValue(password, exp) {
  return `${exp}.${await signCookie(password, exp)}`;
}

export function parseCookies(header) {
  const out = {};
  if (!header) return out;
  for (const part of String(header).split(";")) {
    const i = part.indexOf("=");
    if (i < 0) continue;
    const k = part.slice(0, i).trim();
    if (k && !(k in out)) {
      try {
        out[k] = decodeURIComponent(part.slice(i + 1).trim());
      } catch {
        out[k] = part.slice(i + 1).trim();
      }
    }
  }
  return out;
}

export function cookieSecure(url) {
  return new URL(url).protocol === "https:";
}

export function cookieName(url) {
  return cookieSecure(url) ? `__Host-${COOKIE_BASE}` : COOKIE_BASE;
}

export function setCookieHeader(url, value, maxAge = UNLOCK_DAYS * 86400) {
  const parts = [
    `${cookieName(url)}=${encodeURIComponent(value)}`,
    "Path=/",
    "HttpOnly",
    "SameSite=Lax",
  ];
  if (cookieSecure(url)) parts.push("Secure");
  parts.push(`Max-Age=${Math.max(0, Math.floor(maxAge))}`);
  return parts.join("; ");
}

export const clearCookieHeader = (url) => setCookieHeader(url, "", 0);

export async function cookieUnlocked(request, password, at = now()) {
  if (!password) return false;
  const name = cookieName(request.url);
  const raw = parseCookies(request.headers.get("cookie"))[name] || "";
  const m = /^(\d{10})\.([0-9a-f]{64})$/.exec(raw);
  if (!m || Number(m[1]) < at) return false;
  return timingSafeEqual(m[2], await signCookie(password, Number(m[1])));
}

export function clientIp(request) {
  return request.headers.get("cf-connecting-ip") || request.headers.get("x-real-ip") || "";
}

/** JSON POSTs from the download form must come from this same site. */
export function requireSameOrigin(request) {
  const origin = request.headers.get("origin") || "";
  if (!origin || origin !== new URL(request.url).origin) {
    throw new DownloadError(403, "This request must come from the website.", "forbidden");
  }
}

export function unlockRetryMessage(seconds) {
  const wait = Number(seconds) || 0;
  if (wait > 90) {
    const minutes = Math.max(1, Math.ceil(wait / 60));
    return `Too many attempts. Try again in ${minutes} ${minutes === 1 ? "minute" : "minutes"}.`;
  }
  const s = Math.max(1, Math.ceil(wait));
  return `Too many attempts. Try again in ${s} ${s === 1 ? "second" : "seconds"}.`;
}

export function passwordSet(env) {
  return typeof env?.DOWNLOAD_PASSWORD === "string" && env.DOWNLOAD_PASSWORD.length > 0;
}

export async function isUnlocked(request, env, at = now()) {
  return passwordSet(env) && (await cookieUnlocked(request, env.DOWNLOAD_PASSWORD, at));
}

/**
 * GET /api/download/state, POST /api/download/unlock, POST /api/download/lock.
 * `rate` supplies { rateLimited(bucket, {limit, window}), noteEvent(bucket), retryAfter(bucket, {limit, window}) }.
 * Returns a Response, or null when the path is not a download-gate route.
 */
export async function downloadApi(request, env, url, rate) {
  const p = url.pathname;
  const m = request.method;
  if (p === "/api/download/state" && m === "GET") {
    return jsonBody({ password_set: passwordSet(env), unlocked: await isUnlocked(request, env) });
  }
  if (p === "/api/download/unlock" && m === "POST") return unlock(request, env, url, rate);
  if (p === "/api/download/lock" && m === "POST") {
    requireSameOrigin(request);
    return jsonBody({ ok: true }, 200, { "set-cookie": clearCookieHeader(url) });
  }
  return null;
}

async function unlock(request, env, url, rate) {
  requireSameOrigin(request);
  if (!passwordSet(env)) {
    throw new DownloadError(503, "Downloads aren't open yet", "downloads_closed");
  }
  const ip = clientIp(request) || "?";
  const bucket = `dlpw:${ip}`;
  const wait = rate?.retryAfter
    ? await rate.retryAfter(bucket, UNLOCK_FAILS)
    : (await rate.rateLimited(bucket, UNLOCK_FAILS))
      ? UNLOCK_FAILS.window
      : 0;
  if (wait > 0) {
    throw new DownloadError(429, unlockRetryMessage(wait), "rate_limited", wait);
  }
  let body = {};
  try {
    body = await request.json();
  } catch {
    throw new DownloadError(400, "Bad request.", "invalid_password");
  }
  const pw = typeof body.password === "string" ? body.password : "";
  if (!pw || pw.length > 200) {
    throw new DownloadError(400, "Enter the download password", "invalid_password");
  }
  if (!(await passwordsMatch(pw, env.DOWNLOAD_PASSWORD))) {
    if (rate?.noteEvent) await rate.noteEvent(bucket);
    throw new DownloadError(401, "That isn't the download password", "wrong_password");
  }
  const exp = now() + UNLOCK_DAYS * 86400;
  return jsonBody(
    { ok: true, days: UNLOCK_DAYS },
    200,
    { "set-cookie": setCookieHeader(url, await makeCookieValue(env.DOWNLOAD_PASSWORD, exp)) },
  );
}

/**
 * Whether this request may follow /download/client|server|sums.
 * Browsers without the cookie are sent to the download form; other callers get 401.
 */
export async function requireDownload(request, env) {
  if (await isUnlocked(request, env)) return;
  const html =
    (request.headers.get("accept") || "").includes("text/html") ||
    request.headers.get("sec-fetch-mode") === "navigate";
  if (html) {
    const err = new DownloadError(302, "Enter the download password", passwordSet(env) ? "download_password" : "downloads_closed");
    err.location = "/#download";
    throw err;
  }
  throw new DownloadError(
    401,
    passwordSet(env) ? "Enter the download password" : "Downloads aren't open yet",
    passwordSet(env) ? "download_password" : "downloads_closed",
  );
}

function jsonBody(body, status = 200, headers = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
      "referrer-policy": "no-referrer",
      "x-frame-options": "DENY",
      ...headers,
    },
  });
}
