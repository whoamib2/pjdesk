import test from "node:test";
import assert from "node:assert/strict";
import {
  COOKIE_BASE,
  DownloadError,
  UNLOCK_DAYS,
  UNLOCK_FAILS,
  clearCookieHeader,
  clientIp,
  cookieName,
  cookieUnlocked,
  downloadApi,
  isUnlocked,
  makeCookieValue,
  parseCookies,
  passwordSet,
  passwordsMatch,
  requireDownload,
  requireSameOrigin,
  setCookieHeader,
  signCookie,
  timingSafeEqual,
  unlockRetryMessage,
} from "../src/download-gate.js";

const SITE = "https://pjdesk.com";
const SECRET = "correct-horse-battery";

function req(url, { method = "GET", headers = {}, body } = {}) {
  const h = new Headers(headers);
  const init = { method, headers: h };
  if (body !== undefined) {
    h.set("content-type", "application/json");
    init.body = JSON.stringify(body);
  }
  return new Request(url, init);
}

function memoryRate() {
  const events = new Map();
  const now = () => Math.floor(Date.now() / 1000);
  return {
    events,
    async noteEvent(bucket) {
      const list = events.get(bucket) || [];
      list.push(now());
      events.set(bucket, list);
    },
    async retryAfter(bucket, { limit, window }) {
      const cutoff = now() - window;
      const live = (events.get(bucket) || []).filter((at) => at > cutoff).sort((a, b) => a - b);
      if (live.length < limit) return 0;
      return Math.max(1, live[live.length - limit] + window - now());
    },
  };
}

test("timing-safe compare accepts only identical strings", () => {
  assert.equal(timingSafeEqual("abcd", "abcd"), true);
  assert.equal(timingSafeEqual("abcd", "abce"), false);
  assert.equal(timingSafeEqual("abcd", "abc"), false);
  assert.equal(timingSafeEqual("", ""), true);
});

test("password compare is constant-time on the hashes and uses NFKC", async () => {
  assert.equal(await passwordsMatch(SECRET, SECRET), true);
  assert.equal(await passwordsMatch("wrong", SECRET), false);
  assert.equal(await passwordsMatch("", SECRET), false);
  assert.equal(await passwordsMatch("\u212B", "\u00C5"), true); // Å in two Unicode forms
});

test("a cookie signed with the password verifies, and a changed password or a forged value does not", async () => {
  const exp = Math.floor(Date.now() / 1000) + 3600;
  const value = await makeCookieValue(SECRET, exp);
  const name = cookieName(SITE);
  assert.equal(name, `__Host-${COOKIE_BASE}`);
  const unlocked = await cookieUnlocked(
    req(SITE + "/", { headers: { cookie: `${name}=${value}` } }),
    SECRET,
  );
  assert.equal(unlocked, true);
  const other = await cookieUnlocked(
    req(SITE + "/", { headers: { cookie: `${name}=${value}` } }),
    "different-password",
  );
  assert.equal(other, false);
  const forged = await cookieUnlocked(
    req(SITE + "/", { headers: { cookie: `${name}=${exp}.${"0".repeat(64)}` } }),
    SECRET,
  );
  assert.equal(forged, false);
  const expired = await cookieUnlocked(
    req(SITE + "/", { headers: { cookie: `${name}=${await makeCookieValue(SECRET, exp)}` } }),
    SECRET,
    exp + 1,
  );
  assert.equal(expired, false);
});

test("cookie header is HttpOnly, host-only on HTTPS, and dropped when cleared", () => {
  const set = setCookieHeader(SITE, "token", UNLOCK_DAYS * 86400);
  assert.match(set, /^__Host-pjdesk_dl=/);
  assert.match(set, /HttpOnly/);
  assert.match(set, /Secure/);
  assert.match(set, /SameSite=Lax/);
  assert.match(set, /Path=\//);
  assert.match(set, /Max-Age=2592000/);
  const http = setCookieHeader("http://localhost:8787/", "token");
  assert.match(http, /^pjdesk_dl=/);
  assert.doesNotMatch(http, /Secure/);
  const clear = clearCookieHeader(SITE);
  assert.match(clear, /Max-Age=0/);
  const parsed = parseCookies(`${cookieName(SITE)}=abc; other=1; ${cookieName(SITE)}=second`);
  assert.equal(parsed[cookieName(SITE)], "abc");
  assert.equal(parsed.other, "1");
});

test("unlock POSTs must come from this site", () => {
  assert.throws(
    () => requireSameOrigin(req(SITE + "/api/download/unlock", { method: "POST", headers: { origin: "https://evil.example" } })),
    (e) => e instanceof DownloadError && e.status === 403,
  );
  assert.throws(
    () => requireSameOrigin(req(SITE + "/api/download/unlock", { method: "POST" })),
    (e) => e instanceof DownloadError && e.status === 403,
  );
  requireSameOrigin(
    req(SITE + "/api/download/unlock", { method: "POST", headers: { origin: SITE } }),
  );
});

test("the download state is locked until the right password is given", async () => {
  const rate = memoryRate();
  const env = { DOWNLOAD_PASSWORD: SECRET };
  let res = await downloadApi(req(SITE + "/api/download/state"), env, new URL(SITE + "/api/download/state"), rate);
  assert.equal(res.status, 200);
  assert.deepEqual(await res.json(), { password_set: true, unlocked: false });

  res = await downloadApi(
    req(SITE + "/api/download/unlock", { method: "POST", headers: { origin: SITE }, body: { password: "wrong" } }),
    env,
    new URL(SITE + "/api/download/unlock"),
    rate,
  ).catch((e) => e);
  assert.equal(res.status, 401);
  assert.equal(res.code, "wrong_password");

  res = await downloadApi(
    req(SITE + "/api/download/unlock", {
      method: "POST",
      headers: { origin: SITE, "cf-connecting-ip": "203.0.113.9" },
      body: { password: SECRET },
    }),
    env,
    new URL(SITE + "/api/download/unlock"),
    rate,
  );
  assert.equal(res.status, 200);
  const body = await res.json();
  assert.deepEqual(body, { ok: true, days: 30 });
  const set = res.headers.get("set-cookie");
  assert.match(set, /__Host-pjdesk_dl=/);
  assert.match(set, /HttpOnly/);
  const cookie = set.split(";")[0];

  res = await downloadApi(
    req(SITE + "/api/download/state", { headers: { cookie } }),
    env,
    new URL(SITE + "/api/download/state"),
    rate,
  );
  assert.deepEqual(await res.json(), { password_set: true, unlocked: true });
  assert.equal(await isUnlocked(req(SITE + "/", { headers: { cookie } }), env), true);
});

test("a wrong password is rate-limited after 10 tries from one address", async () => {
  const rate = memoryRate();
  const env = { DOWNLOAD_PASSWORD: SECRET };
  const url = new URL(SITE + "/api/download/unlock");
  const attempt = () =>
    downloadApi(
      req(url, {
        method: "POST",
        headers: { origin: SITE, "cf-connecting-ip": "198.51.100.10" },
        body: { password: "nope" },
      }),
      env,
      url,
      rate,
    );
  for (let i = 0; i < UNLOCK_FAILS.limit; i++) {
    await assert.rejects(attempt, (e) => e instanceof DownloadError && e.code === "wrong_password");
  }
  await assert.rejects(
    attempt,
    (e) => e instanceof DownloadError && e.status === 429 && e.code === "rate_limited" && e.retryAfter > 0,
  );
  assert.equal(UNLOCK_FAILS.limit, 10);
  assert.equal(UNLOCK_FAILS.window, 15 * 60);
  assert.equal(unlockRetryMessage(30), "Too many attempts. Try again in 30 seconds.");
  assert.equal(unlockRetryMessage(91), "Too many attempts. Try again in 2 minutes.");
});

test("without a secret, downloads stay closed and unlock is 503", async () => {
  const rate = memoryRate();
  const url = new URL(SITE + "/api/download/state");
  const res = await downloadApi(req(url), {}, url, rate);
  assert.deepEqual(await res.json(), { password_set: false, unlocked: false });
  assert.equal(passwordSet({}), false);
  await assert.rejects(
    downloadApi(
      req(SITE + "/api/download/unlock", { method: "POST", headers: { origin: SITE }, body: { password: "x" } }),
      {},
      new URL(SITE + "/api/download/unlock"),
      rate,
    ),
    (e) => e instanceof DownloadError && e.status === 503 && e.code === "downloads_closed",
  );
});

test("a browser hitting /download/* without the cookie is sent to the form; other callers get 401", async () => {
  const env = { DOWNLOAD_PASSWORD: SECRET };
  await assert.rejects(
    requireDownload(req(SITE + "/download/client", { headers: { accept: "text/html" } }), env),
    (e) => e instanceof DownloadError && e.status === 302 && e.location === "/#download" && e.code === "download_password",
  );
  await assert.rejects(
    requireDownload(req(SITE + "/download/client"), env),
    (e) => e instanceof DownloadError && e.status === 401 && e.code === "download_password",
  );
  const exp = Math.floor(Date.now() / 1000) + 60;
  const cookie = `${cookieName(SITE)}=${await makeCookieValue(SECRET, exp)}`;
  await requireDownload(req(SITE + "/download/client", { headers: { cookie } }), env);
});

test("client IP comes from Cloudflare, and lock clears the cookie", async () => {
  assert.equal(clientIp(req(SITE + "/", { headers: { "cf-connecting-ip": "192.0.2.5" } })), "192.0.2.5");
  const res = await downloadApi(
    req(SITE + "/api/download/lock", { method: "POST", headers: { origin: SITE } }),
    { DOWNLOAD_PASSWORD: SECRET },
    new URL(SITE + "/api/download/lock"),
    memoryRate(),
  );
  assert.equal(res.status, 200);
  assert.match(res.headers.get("set-cookie"), /Max-Age=0/);
});

test("HMAC signatures for two expiries differ, and signCookie is 64 hex chars", async () => {
  const a = await signCookie(SECRET, 1_700_000_000);
  const b = await signCookie(SECRET, 1_700_000_001);
  assert.equal(a.length, 64);
  assert.notEqual(a, b);
  assert.match(a, /^[0-9a-f]{64}$/);
});
