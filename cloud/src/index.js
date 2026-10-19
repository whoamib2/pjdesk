// PJDesk Cloud: accounts and encrypted server sync, plus the pjdesk.com website.
//
// Privacy model — the service never learns a customer's password or their data:
//   master   = PBKDF2-SHA256(password, SHA-256("pjdesk-cloud-v1\0" + email), 600000)
//   auth_key = HMAC-SHA256(master, "pjdesk-cloud-auth")      → sent to sign in
//   sync_key = HMAC-SHA256(master, "pjdesk-cloud-sync-key")  → stays on the server,
//              seals the server copy (ChaCha20-Poly1305) before upload
// The service stores a salted PBKDF2 hash of auth_key and the sealed bytes.
//
// Routes:
//   POST /api/signup            {email, auth_key}
//   POST /api/login             {email, auth_key, device} → {token, ...}
//   POST /api/logout            (Bearer)
//   GET  /api/me                (Bearer)
//   POST /api/password          (Bearer) {old_auth_key, new_auth_key}
//   GET  /api/sync              (Bearer, sync on) sealed copy; ETag / If-None-Match
//   PUT  /api/sync              (Bearer, sync on) sealed copy
//                               If-Match / If-None-Match: * → 412 when the
//                               precondition fails. Omitting both is the
//                               unconditional put older servers send.
//   GET  /api/sync/health       (Bearer, sync on) { peers: [...] }
//   PUT  /api/sync/health       (Bearer, sync on) one server's version/role
//   GET  /api/admin/accounts    (Bearer, admin)
//   GET  /api/admin/overview    (Bearer, admin)
//   GET  /api/admin/servers     (Bearer, admin) health reports + sign-ins
//   GET  /api/admin/audit       (Bearer, admin)
//   POST /api/admin/plan        (Bearer, admin) {email, plan, expires_at, note}
//   POST /api/admin/sync        (Bearer, admin) {email, enabled}  (audited; same as a plan grant)
//   GET  /api/release           latest release info (from GitHub)
//   GET  /download/{client,server,sums}  → the latest release's files
// Everything else is the static website in ./public.

import { effectivePlan, isPlan } from "./plans.js";
import { route as networkRoute } from "./network.js";

const MAX_SYNC_BYTES = 8 * 1024 * 1024;
const AUTH_HASH_ITERATIONS = 100000; // the Workers PBKDF2 maximum
const LOGIN_FAILS = { limit: 10, window: 15 * 60 };
const LOGIN_IP = { limit: 30, window: 15 * 60 };
const SIGNUPS = { limit: 5, window: 60 * 60 };
const SESSION_IDLE = 90 * 24 * 3600;
const SESSION_ABSOLUTE = 365 * 24 * 3600;
const SEAL_MAGIC = "PJSYNC1\0";

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    try {
      if (url.pathname.startsWith("/api/")) return await api(request, env, url, ctx);
      if (url.pathname.startsWith("/download/")) return await download(env, url);
      return env.ASSETS.fetch(request);
    } catch (e) {
      if (e instanceof HttpError) return json({ error: e.message }, e.status);
      console.error(e && e.stack ? e.stack : e);
      return json({ error: "Something went wrong. Please try again." }, 500);
    }
  },
};

class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

// ---------------------------------------------------------------------------
// Helpers

const encoder = new TextEncoder();
const now = () => Math.floor(Date.now() / 1000);

function json(body, status = 200, headers = {}) {
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

function toHex(buf) {
  return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function fromHex(s) {
  const out = new Uint8Array(s.length / 2);
  for (let i = 0; i < out.length; i++) out[i] = parseInt(s.substr(i * 2, 2), 16);
  return out;
}

function randomHex(n) {
  return toHex(crypto.getRandomValues(new Uint8Array(n)));
}

function base64url(bytes) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

async function sha256Hex(text) {
  return toHex(await crypto.subtle.digest("SHA-256", encoder.encode(text)));
}

async function hashAuthKey(authKeyHex, saltHex) {
  const key = await crypto.subtle.importKey("raw", fromHex(authKeyHex), "PBKDF2", false, ["deriveBits"]);
  const bits = await crypto.subtle.deriveBits(
    { name: "PBKDF2", hash: "SHA-256", salt: fromHex(saltHex), iterations: AUTH_HASH_ITERATIONS },
    key,
    256,
  );
  return toHex(bits);
}

function sameHex(a, b) {
  if (typeof a !== "string" || typeof b !== "string" || a.length !== b.length) return false;
  return crypto.subtle.timingSafeEqual(encoder.encode(a), encoder.encode(b));
}

const normEmail = (e) => String(e || "").trim().toLowerCase();
const validEmail = (e) => e.length <= 254 && /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(e);
const validKey = (k) => typeof k === "string" && /^[0-9a-f]{64}$/.test(k);

async function readJson(request) {
  try {
    return await request.json();
  } catch {
    throw new HttpError(400, "Bad request.");
  }
}

function isAdmin(env, email) {
  return String(env.ADMIN_EMAILS || "")
    .split(",")
    .map(normEmail)
    .filter(Boolean)
    .includes(email);
}

async function rateLimited(env, bucket, { limit, window }) {
  const row = await env.DB.prepare("SELECT COUNT(*) AS n FROM rate_events WHERE bucket = ? AND at > ?")
    .bind(bucket, now() - window)
    .first();
  return (row?.n || 0) >= limit;
}

async function noteEvent(env, bucket) {
  const stmts = [env.DB.prepare("INSERT INTO rate_events (bucket, at) VALUES (?, ?)").bind(bucket, now())];
  if (Math.random() < 0.05) stmts.push(env.DB.prepare("DELETE FROM rate_events WHERE at < ?").bind(now() - 86400));
  await env.DB.batch(stmts);
}

/** The signed-in account for a Bearer token. */
async function session(request, env) {
  const m = (request.headers.get("authorization") || "").match(/^Bearer\s+([A-Za-z0-9_-]{20,128})$/);
  if (!m) throw new HttpError(401, "Please sign in.");
  const tokenHash = await sha256Hex(m[1]);
  const row = await env.DB.prepare(
    `SELECT s.account_id, s.created_at, s.last_used_at, a.email, a.sync_enabled,
            a.plan, a.plan_expires_at
       FROM sessions s JOIN accounts a ON a.id = s.account_id
      WHERE s.token_hash = ?`,
  )
    .bind(tokenHash)
    .first();
  if (!row) throw new HttpError(401, "You were signed out. Please sign in again.");
  const t = now();
  if (t - row.created_at > SESSION_ABSOLUTE || t - row.last_used_at > SESSION_IDLE) {
    await env.DB.prepare("DELETE FROM sessions WHERE token_hash = ?").bind(tokenHash).run();
    throw new HttpError(401, "You were signed out. Please sign in again.");
  }
  if (t - row.last_used_at > 3600) {
    await env.DB.prepare("UPDATE sessions SET last_used_at = ? WHERE token_hash = ?").bind(now(), tokenHash).run();
  }
  const admin = isAdmin(env, row.email);
  const eff = effectivePlan({
    plan: row.plan,
    expiresAt: row.plan_expires_at,
    isAdmin: admin,
    now: t,
  });
  if (eff.expired) {
    await env.DB.batch([
      env.DB.prepare(
        "UPDATE accounts SET plan = 'free', sync_enabled = 0, plan_expires_at = NULL WHERE id = ?",
      ).bind(row.account_id),
      env.DB.prepare(
        `INSERT INTO plan_audit (at, actor, email, old_plan, new_plan, expires_at, note)
         VALUES (?, 'expiry', ?, 'cloud', 'free', ?, 'Cloud Sync grant expired')`,
      ).bind(t, row.email, row.plan_expires_at),
    ]);
  }
  return { accountId: row.account_id, email: row.email, admin, sync: eff.sync, plan: eff.plan, tokenHash };
}

const objectKey = (accountId) => `sync/${accountId}/server.sync`;

// ---------------------------------------------------------------------------
// API

async function api(request, env, url, ctx) {
  const p = url.pathname;
  const m = request.method;
  if (p === "/api/health") return json({ ok: true });
  if (p === "/api/signup" && m === "POST") return signup(request, env);
  if (p === "/api/login" && m === "POST") return login(request, env);
  if (p === "/api/logout" && m === "POST") return logout(request, env);
  if (p === "/api/me" && m === "GET") return me(request, env);
  if (p === "/api/password" && m === "POST") return changePassword(request, env);
  if (p === "/api/sync" && m === "GET") return syncGet(request, env);
  if (p === "/api/sync" && m === "PUT") return syncPut(request, env);
  if (p === "/api/sync/health" && m === "GET") return syncHealthGet(request, env);
  if (p === "/api/sync/health" && m === "PUT") return syncHealthPut(request, env);
  if (p === "/api/admin/accounts" && m === "GET") return adminAccounts(request, env);
  if (p === "/api/admin/overview" && m === "GET") return adminOverview(request, env);
  if (p === "/api/admin/servers" && m === "GET") return adminServers(request, env);
  if (p === "/api/admin/audit" && m === "GET") return adminAudit(request, env);
  if (p === "/api/admin/plan" && m === "POST") return adminSetPlan(request, env);
  if (p === "/api/admin/sync" && m === "POST") return adminSetSync(request, env);
  if (p.startsWith("/api/network")) {
    const handled = await networkRoute(request, env, url);
    if (handled) return handled;
  }
  if (p === "/api/release" && m === "GET") {
    return json(await latestRelease(env), 200, { "cache-control": "public, max-age=300" });
  }
  throw new HttpError(404, "Not found.");
}

async function signup(request, env) {
  const { email: rawEmail, auth_key } = await readJson(request);
  const email = normEmail(rawEmail);
  if (!validEmail(email)) throw new HttpError(400, "Enter a valid email address.");
  if (!validKey(auth_key)) throw new HttpError(400, "Bad request.");
  const ip = request.headers.get("cf-connecting-ip") || "local";
  if (await rateLimited(env, `signup:${ip}`, SIGNUPS)) {
    throw new HttpError(429, "Too many new accounts from your network. Try again later.");
  }
  const salt = randomHex(16);
  const hash = await hashAuthKey(auth_key, salt);
  // Count the attempt before insert so a duplicate address burns a slot too.
  // The response is the same whether or not the address was new: saying
  // "already exists" lets anyone enumerate accounts. There is no mailer, so
  // this cannot be closed completely — a follow-up sign-in with the same
  // password still succeeds only for an address the caller just created.
  await noteEvent(env, `signup:${ip}`);
  try {
    await env.DB.prepare(
      "INSERT INTO accounts (id, email, auth_hash, auth_salt, sync_enabled, created_at) VALUES (?, ?, ?, ?, 0, ?)",
    )
      .bind(randomHex(16), email, hash, salt, now())
      .run();
  } catch (e) {
    if (String(e).includes("UNIQUE")) return json({ ok: true }, 201);
    throw e;
  }
  return json({ ok: true }, 201);
}

async function login(request, env) {
  const { email: rawEmail, auth_key, device } = await readJson(request);
  const email = normEmail(rawEmail);
  if (!validEmail(email) || !validKey(auth_key)) throw new HttpError(400, "Enter your email and password.");
  const ip = request.headers.get("cf-connecting-ip") || "local";
  if (
    (await rateLimited(env, `login:${email}`, LOGIN_FAILS)) ||
    (await rateLimited(env, `login-ip:${ip}`, LOGIN_IP))
  ) {
    throw new HttpError(429, "Too many failed sign-ins. Wait 15 minutes and try again.");
  }
  const acct = await env.DB.prepare("SELECT id, auth_hash, auth_salt, sync_enabled FROM accounts WHERE email = ?")
    .bind(email)
    .first();
  // Hash even for an unknown email, so timing doesn't reveal which exist.
  const hash = await hashAuthKey(auth_key, acct ? acct.auth_salt : "00".repeat(16));
  if (!acct || !sameHex(hash, acct.auth_hash)) {
    await noteEvent(env, `login:${email}`);
    await noteEvent(env, `login-ip:${ip}`);
    throw new HttpError(401, "Wrong email or password.");
  }
  const token = base64url(crypto.getRandomValues(new Uint8Array(32)));
  const t = now();
  await env.DB.batch([
    env.DB.prepare(
      "INSERT INTO sessions (token_hash, account_id, device, created_at, last_used_at) VALUES (?, ?, ?, ?, ?)",
    ).bind(await sha256Hex(token), acct.id, String(device || "").slice(0, 80), t, t),
    env.DB.prepare("UPDATE accounts SET last_login_at = ? WHERE id = ?").bind(t, acct.id),
  ]);
  const admin = isAdmin(env, email);
  return json({ token, email, sync_enabled: admin || !!acct.sync_enabled, is_admin: admin });
}

async function logout(request, env) {
  const s = await session(request, env);
  await env.DB.prepare("DELETE FROM sessions WHERE token_hash = ?").bind(s.tokenHash).run();
  return json({ ok: true });
}

async function me(request, env) {
  const s = await session(request, env);
  const meta = await env.DB.prepare("SELECT size, updated_at FROM sync_meta WHERE account_id = ?").bind(s.accountId).first();
  const devices = await env.DB.prepare(
    "SELECT device, created_at, last_used_at FROM sessions WHERE account_id = ? ORDER BY last_used_at DESC LIMIT 25",
  )
    .bind(s.accountId)
    .all();
  return json({ email: s.email, sync_enabled: s.sync, is_admin: s.admin, sync: meta || null, devices: devices.results });
}

/** Changes the password. The sealed copy was made with the old password, so
 * it is removed; the main server uploads a new one once it signs in again.
 * Other sign-ins (servers, browsers) are signed out. */
async function changePassword(request, env) {
  const s = await session(request, env);
  const { old_auth_key, new_auth_key } = await readJson(request);
  if (!validKey(old_auth_key) || !validKey(new_auth_key)) throw new HttpError(400, "Bad request.");
  if (await rateLimited(env, `login:${s.email}`, LOGIN_FAILS)) {
    throw new HttpError(429, "Too many failed attempts. Wait 15 minutes and try again.");
  }
  const acct = await env.DB.prepare("SELECT auth_hash, auth_salt FROM accounts WHERE id = ?").bind(s.accountId).first();
  if (!sameHex(await hashAuthKey(old_auth_key, acct.auth_salt), acct.auth_hash)) {
    await noteEvent(env, `login:${s.email}`);
    throw new HttpError(401, "Your current password is wrong.");
  }
  const salt = randomHex(16);
  const hash = await hashAuthKey(new_auth_key, salt);
  await env.DB.batch([
    env.DB.prepare("UPDATE accounts SET auth_hash = ?, auth_salt = ? WHERE id = ?").bind(hash, salt, s.accountId),
    env.DB.prepare("DELETE FROM sessions WHERE account_id = ? AND token_hash != ?").bind(s.accountId, s.tokenHash),
    env.DB.prepare("DELETE FROM sync_meta WHERE account_id = ?").bind(s.accountId),
  ]);
  await env.SYNC.delete(objectKey(s.accountId));
  return json({ ok: true });
}

function requireSync(s) {
  if (!s.sync) {
    throw new HttpError(403, "Cloud sync isn't turned on for this account yet.");
  }
}

async function syncGet(request, env) {
  const s = await session(request, env);
  requireSync(s);
  const obj = await env.SYNC.get(objectKey(s.accountId), { onlyIf: request.headers });
  if (obj === null) return json({ error: "Nothing has been uploaded yet." }, 404);
  if (obj.size > MAX_SYNC_BYTES) return json({ error: "That server copy is too large." }, 413);
  if (!("body" in obj) || !obj.body) {
    return new Response(null, { status: 304, headers: { etag: obj.httpEtag, "cache-control": "no-store" } });
  }
  return new Response(obj.body, {
    headers: { "content-type": "application/octet-stream", etag: obj.httpEtag, "cache-control": "no-store" },
  });
}

/** Reads a request body, stopping once it passes `max` bytes. */
async function readLimited(request, max) {
  const declared = Number(request.headers.get("content-length") || 0);
  if (Number.isFinite(declared) && declared > max) {
    throw new HttpError(413, "That server copy is too large.");
  }
  if (!request.body) throw new HttpError(400, "Bad request.");
  const reader = request.body.getReader();
  const chunks = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      await reader.cancel();
      throw new HttpError(413, "That server copy is too large.");
    }
    chunks.push(value);
  }
  const buf = new Uint8Array(total);
  let off = 0;
  for (const chunk of chunks) {
    buf.set(chunk, off);
    off += chunk.byteLength;
  }
  return buf;
}

function etagNorm(s) {
  return String(s || "")
    .trim()
    .replace(/^W\//i, "")
    .replace(/^"+|"+$/g, "");
}

async function syncPut(request, env) {
  const s = await session(request, env);
  requireSync(s);
  const buf = await readLimited(request, MAX_SYNC_BYTES);
  // The service can't read the copy, but it must at least be a sealed one.
  const head = buf.subarray(0, SEAL_MAGIC.length);
  if (buf.byteLength < 64 || String.fromCharCode(...head) !== SEAL_MAGIC) {
    throw new HttpError(400, "That isn't a sealed PJDesk server copy.");
  }
  const key = objectKey(s.accountId);
  const ifMatch = request.headers.get("if-match");
  const ifNone = request.headers.get("if-none-match");
  // No precondition header: the unconditional put a 0.3.x server sends.
  let onlyIf;
  if (ifMatch) {
    onlyIf = request.headers;
  } else if (ifNone && etagNorm(ifNone) === "*") {
    const existing = await env.SYNC.head(key);
    if (existing) return json({ error: "The cloud copy changed." }, 412);
    onlyIf = request.headers;
  }
  const obj = await env.SYNC.put(key, buf.buffer, {
    httpMetadata: { contentType: "application/octet-stream" },
    onlyIf,
  });
  // R2 returns null when onlyIf fails. That is the fence: the other server won.
  if (!obj) return json({ error: "The cloud copy changed." }, 412);
  await env.DB.prepare(
    `INSERT INTO sync_meta (account_id, size, etag, updated_at) VALUES (?, ?, ?, ?)
     ON CONFLICT(account_id) DO UPDATE SET size = excluded.size, etag = excluded.etag, updated_at = excluded.updated_at`,
  )
    .bind(s.accountId, buf.byteLength, obj.httpEtag, now())
    .run();
  return json({ ok: true, etag: obj.httpEtag }, 200, { etag: obj.httpEtag });
}

function healthKey(accountId, instance) {
  return `${objectKey(accountId)}/health/${instance}`;
}

function validInstance(id) {
  return typeof id === "string" && /^[0-9a-f]{32}$/.test(id);
}

async function syncHealthPut(request, env) {
  const s = await session(request, env);
  requireSync(s);
  const body = await readJson(request);
  if (!validInstance(body.instance)) throw new HttpError(400, "Bad request.");
  const record = {
    instance: body.instance,
    name: String(body.name || "").slice(0, 64),
    version: String(body.version || "").slice(0, 32),
    role: body.role === "active" ? "active" : "standby",
    epoch: Number.isFinite(body.epoch) ? body.epoch : 0,
    healthy: !!body.healthy,
    at: now(),
  };
  await env.SYNC.put(healthKey(s.accountId, record.instance), JSON.stringify(record), {
    httpMetadata: { contentType: "application/json" },
  });
  await env.DB.prepare(
    `INSERT INTO server_health (account_id, instance, name, version, role, epoch, healthy, reported_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?)
     ON CONFLICT(account_id, instance) DO UPDATE SET
       name = excluded.name,
       version = excluded.version,
       role = excluded.role,
       epoch = excluded.epoch,
       healthy = excluded.healthy,
       reported_at = excluded.reported_at`,
  )
    .bind(
      s.accountId,
      record.instance,
      record.name,
      record.version,
      record.role,
      record.epoch,
      record.healthy ? 1 : 0,
      record.at,
    )
    .run();
  return json({ ok: true });
}

async function syncHealthGet(request, env) {
  const s = await session(request, env);
  requireSync(s);
  const listed = await env.SYNC.list({ prefix: `${objectKey(s.accountId)}/health/` });
  const peers = [];
  for (const obj of listed.objects || []) {
    const got = await env.SYNC.get(obj.key);
    if (!got) continue;
    try {
      peers.push(await got.json());
    } catch {
      // A damaged row is skipped; it must not hide the others.
    }
  }
  return json({ peers });
}

async function requireAdmin(request, env) {
  const s = await session(request, env);
  if (!s.admin) throw new HttpError(403, "This page is for PJDesk administrators.");
  return s;
}

function accountView(env, r) {
  const admin = isAdmin(env, r.email);
  const eff = effectivePlan({
    plan: r.plan,
    expiresAt: r.plan_expires_at,
    isAdmin: admin,
    now: now(),
  });
  return {
    email: r.email,
    created_at: r.created_at,
    last_login_at: r.last_login_at,
    signed_in: r.signed_in || 0,
    size: r.size,
    synced_at: r.synced_at,
    is_admin: admin,
    plan: eff.plan,
    plan_expires_at: eff.expired ? null : r.plan_expires_at,
    plan_note: r.plan_note || "",
    sync_enabled: eff.sync,
    expired: eff.expired,
  };
}

async function adminAccounts(request, env) {
  await requireAdmin(request, env);
  const rows = await env.DB.prepare(
    `SELECT a.email, a.sync_enabled, a.plan, a.plan_expires_at, a.plan_note, a.created_at, a.last_login_at,
            m.size, m.updated_at AS synced_at,
            (SELECT COUNT(*) FROM sessions s WHERE s.account_id = a.id) AS signed_in
       FROM accounts a LEFT JOIN sync_meta m ON m.account_id = a.id
      ORDER BY a.created_at DESC LIMIT 1000`,
  ).all();
  return json({ accounts: rows.results.map((r) => accountView(env, r)) });
}

async function adminOverview(request, env) {
  await requireAdmin(request, env);
  const accounts = await env.DB.prepare(
    "SELECT email, plan, plan_expires_at, sync_enabled, created_at FROM accounts",
  ).all();
  const at = now();
  const week = at - 7 * 86400;
  const day = at - 86400;
  let cloud = 0;
  let free = 0;
  let fresh = 0;
  for (const r of accounts.results) {
    const eff = effectivePlan({
      plan: r.plan,
      expiresAt: r.plan_expires_at,
      isAdmin: isAdmin(env, r.email),
      now: at,
    });
    if (eff.plan === "cloud") cloud += 1;
    else free += 1;
    if (Number(r.created_at) >= week) fresh += 1;
  }
  const servers = await env.DB.prepare("SELECT COUNT(*) AS n FROM server_health").first();
  const signins = await env.DB.prepare("SELECT COUNT(*) AS n FROM sessions").first();
  const active = await env.DB.prepare("SELECT COUNT(*) AS n FROM sessions WHERE last_used_at >= ?").bind(day).first();
  const audit = await env.DB.prepare("SELECT COUNT(*) AS n FROM plan_audit").first();
  const serverList = await env.DB.prepare(
    `SELECT a.email, h.name, h.version, h.role, h.healthy, h.reported_at
       FROM server_health h JOIN accounts a ON a.id = h.account_id
      ORDER BY h.reported_at DESC LIMIT 8`,
  ).all();
  const recentAudit = await env.DB.prepare(
    `SELECT at, actor, email, old_plan, new_plan, note FROM plan_audit ORDER BY id DESC LIMIT 8`,
  ).all();
  const recentSignins = await env.DB.prepare(
    `SELECT a.email, s.device, s.last_used_at
       FROM sessions s JOIN accounts a ON a.id = s.account_id
      ORDER BY s.last_used_at DESC LIMIT 8`,
  ).all();
  return json({
    accounts: accounts.results.length,
    cloud,
    free,
    new_accounts_7d: fresh,
    servers: servers?.n || 0,
    signins: signins?.n || 0,
    active_today: active?.n || 0,
    audit: audit?.n || 0,
    server_list: serverList.results,
    recent_audit: recentAudit.results,
    recent_signins: recentSignins.results,
  });
}

async function adminServers(request, env) {
  await requireAdmin(request, env);
  const servers = await env.DB.prepare(
    `SELECT a.email, h.name, h.version, h.role, h.epoch, h.healthy, h.reported_at, h.instance
       FROM server_health h JOIN accounts a ON a.id = h.account_id
      ORDER BY h.reported_at DESC LIMIT 500`,
  ).all();
  const signins = await env.DB.prepare(
    `SELECT a.email, s.device, s.created_at, s.last_used_at
       FROM sessions s JOIN accounts a ON a.id = s.account_id
      ORDER BY s.last_used_at DESC LIMIT 500`,
  ).all();
  return json({
    servers: servers.results,
    signins: signins.results,
    note: "Desktop client versions are on each PJDesk Server window. This page lists servers that reported a Cloud Sync health record, and account sign-ins. Sign-ins do not include a client version.",
  });
}

async function adminAudit(request, env) {
  await requireAdmin(request, env);
  const rows = await env.DB.prepare(
    `SELECT at, actor, email, old_plan, new_plan, expires_at, note
       FROM plan_audit ORDER BY id DESC LIMIT 200`,
  ).all();
  return json({ audit: rows.results });
}

async function adminSetPlan(request, env) {
  const admin = await requireAdmin(request, env);
  const body = await readJson(request);
  const email = normEmail(body.email);
  if (!isPlan(body.plan)) throw new HttpError(400, "Plan must be free or cloud.");
  const note = String(body.note || "").slice(0, 500);
  let expires = null;
  if (body.expires_at != null && body.expires_at !== "") {
    expires = Number(body.expires_at);
    if (!Number.isFinite(expires) || expires < 0) throw new HttpError(400, "Expiry must be a time.");
  }
  const row = await env.DB.prepare(
    "SELECT id, plan, plan_expires_at, email FROM accounts WHERE email = ?",
  )
    .bind(email)
    .first();
  if (!row) throw new HttpError(404, "No account with that email.");
  const sync = body.plan === "cloud" && !(expires && expires <= now()) ? 1 : 0;
  await env.DB.batch([
    env.DB.prepare(
      "UPDATE accounts SET plan = ?, plan_expires_at = ?, plan_note = ?, sync_enabled = ? WHERE id = ?",
    ).bind(body.plan, expires, note, sync, row.id),
    env.DB.prepare(
      `INSERT INTO plan_audit (at, actor, email, old_plan, new_plan, expires_at, note)
       VALUES (?, ?, ?, ?, ?, ?, ?)`,
    ).bind(now(), admin.email, email, row.plan || "free", body.plan, expires, note),
  ]);
  return json({ ok: true });
}

async function adminSetSync(request, env) {
  const body = await readJson(request);
  const email = normEmail(body.email);
  return adminSetPlan(
    new Request(request.url, {
      method: "POST",
      headers: request.headers,
      body: JSON.stringify({
        email,
        plan: body.enabled ? "cloud" : "free",
        expires_at: null,
        note: "Changed from the Cloud Sync switch",
      }),
    }),
    env,
  );
}

// ---------------------------------------------------------------------------
// Releases and downloads (so the website never needs editing per release)

// The latest version comes from github.com's /releases/latest redirect, not the
// GitHub API: the API's anonymous limit is shared by every Worker on the same
// Cloudflare address and runs out. The installer names follow a fixed pattern.
const RELEASE_TTL_MS = 10 * 60 * 1000;
let releaseMemo = null; // { info, at } — kept for the life of this Worker instance

function releaseInfo(repo, tag) {
  const version = tag.replace(/^v/, "");
  const base = `https://github.com/${repo}/releases/download/${tag}`;
  return {
    version,
    client: `${base}/PJDesk-Setup-${version}.exe`,
    server: `${base}/PJDesk-Server-Setup-${version}.exe`,
    sums: `${base}/SHA256SUMS`,
    notes: `https://github.com/${repo}/releases/tag/${tag}`,
  };
}

async function latestTag(repo) {
  const res = await fetch(`https://github.com/${repo}/releases/latest`, {
    method: "HEAD",
    redirect: "manual",
    headers: { "user-agent": "pjdesk-cloud" },
  });
  const m = /\/releases\/tag\/([^/?#]+)/.exec(res.headers.get("location") || "");
  if (m) return decodeURIComponent(m[1]);
  // Fallback: the API (may be rate limited).
  const api = await fetch(`https://api.github.com/repos/${repo}/releases/latest`, {
    headers: { "user-agent": "pjdesk-cloud", accept: "application/vnd.github+json" },
  });
  if (api.ok) {
    const r = await api.json();
    if (r.tag_name) return String(r.tag_name);
  }
  return null;
}

function safeRepo(repo) {
  return typeof repo === "string" && /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo);
}

/** Release tags become part of a redirect URL, so they stay a single path segment. */
function safeTag(tag) {
  return typeof tag === "string" && /^v?[0-9A-Za-z][0-9A-Za-z._+-]{0,63}$/.test(tag) && !tag.includes("..");
}

async function latestRelease(env) {
  if (!safeRepo(env.RELEASE_REPO)) throw new HttpError(500, "Release repository is not configured.");
  if (releaseMemo && Date.now() - releaseMemo.at < RELEASE_TTL_MS) return releaseMemo.info;
  let tag = null;
  try {
    tag = await latestTag(env.RELEASE_REPO);
  } catch (e) {
    console.error("release lookup failed:", e && e.message);
  }
  if (!safeTag(tag)) {
    if (releaseMemo) return releaseMemo.info; // stale is better than nothing
    throw new HttpError(502, "Couldn't reach GitHub for the latest release.");
  }
  const info = releaseInfo(env.RELEASE_REPO, tag);
  releaseMemo = { info, at: Date.now() };
  return info;
}

async function download(env, url) {
  const what = url.pathname.split("/")[2] || "";
  let info = null;
  try {
    info = await latestRelease(env);
  } catch {
    // Fall back to the releases page below.
  }
  const target = info && { client: info.client, server: info.server, sums: info.sums }[what];
  const fallback = safeRepo(env.RELEASE_REPO)
    ? `https://github.com/${env.RELEASE_REPO}/releases/latest`
    : "https://pjdesk.com/";
  return Response.redirect(target || fallback, 302);
}
