// Private-network access for PJDesk 0.6.0.
//
// A short-lived Ed25519 token, signed by a key that lives only in D1, names
// the account, the network, the server fingerprints, the device, and the
// expiry. The PJDesk server pins the public key and refuses register, lookup,
// and relay without a valid token once the owner turns that on.
//
// The canonical body must match crates/pjdesk-proto/src/access.rs and the
// Android client:
//   PJN1\n{email}\n{network id}\n{sorted fingerprints}\n{device id}\n{exp}\n

import { JOIN_TTL_SEC, SUPPORT_TTL_SEC, joinRedeemable, nextGuess, randomDigits } from "./codes.js";

const TOKEN_TTL = 15 * 60;
const REFRESH_TTL = 365 * 24 * 3600;
const ENROLL_TTL = 60 * 60;

const encoder = new TextEncoder();
const now = () => Math.floor(Date.now() / 1000);

export class HttpError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

function json(body, status = 200) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
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

const normEmail = (e) => String(e || "").trim().toLowerCase();
const validEmail = (e) => e.length <= 254 && /^[^\s@]+@[^\s@]+\.[^\s@]+$/.test(e);

export function normalizeFingerprint(text) {
  const raw = String(text || "").trim().replace(/^sha256:/i, "");
  return raw.toLowerCase().replace(/[^0-9a-f]/g, "");
}

export function canonical({ email, networkId, fingerprints, deviceId, exp }) {
  const fps = [...new Set((fingerprints || []).map(normalizeFingerprint).filter(Boolean))].sort();
  return `PJN1\n${normEmail(email)}\n${String(networkId).trim()}\n${fps.join(",")}\n${String(deviceId).trim()}\n${exp}\n`;
}

/** Mirrors the server's admit() for the pure cases the Worker can see. */
/** Why a network key cannot be redeemed. Null means it can. Matches pjdesk-proto. */
export const NO_SERVER_MESSAGE =
  "Your network has no server yet. Sign in to PJDesk Server on your server computer, or set it on pjdesk.com → Account → My network.";

function hostOf(address) {
  const text = String(address || "").trim();
  if (!text) return "";
  if (text.startsWith("[")) {
    const end = text.indexOf("]");
    return end > 1 ? text.slice(1, end) : "";
  }
  const v4 = text.match(/^(\d{1,3}(?:\.\d{1,3}){3})(?::\d+)?$/);
  if (v4) return v4[1];
  if (text.includes(":") && /^\d+$/.test(text.slice(text.lastIndexOf(":") + 1))) {
    return text.slice(0, text.lastIndexOf(":"));
  }
  return text;
}

/** A hostname is not private. A LAN, loopback, link-local, or CGNAT address is. */
export function isPrivateAddress(address) {
  const host = hostOf(address);
  if (!host) return false;
  const m = host.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (m) {
    const n = m.slice(1).map(Number);
    if (n.some((part) => part > 255)) return true;
    const [a, b] = n;
    if (a === 0 || a === 10 || a === 127 || a === 255) return true;
    if (a === 169 && b === 254) return true;
    if (a === 172 && b >= 16 && b <= 31) return true;
    if (a === 192 && b === 168) return true;
    if (a === 100 && b >= 64 && b <= 127) return true;
    return false;
  }
  const lower = host.toLowerCase();
  if (lower === "::1" || lower === "::") return true;
  if (lower.startsWith("fe80:") || lower.startsWith("fc") || lower.startsWith("fd")) return true;
  return false;
}

/** True only for an explicit on. `0`, `"0"`, null, and a missing value stay off. */
export function requireAccountsOn(value) {
  return value === true || value === 1;
}

function lanText(lan) {
  const parts = Array.isArray(lan) ? lan : String(lan || "").split(",");
  return parts
    .map((part) => String(part || "").trim())
    .filter(Boolean)
    .slice(0, 8)
    .join(",")
    .slice(0, 500);
}

/**
 * What an owner's server may write. A non-owner never writes. A different
 * fingerprint is kept unless `replace` is set. A different address is kept
 * unless this same server instance already owns the slot, or `replace` is set.
 * A private address is kept unless `confirm` is set. A shared certificate
 * (the standby holds the active server's key) does not clear the other slot.
 */
export function planRegistration({
  owner,
  role,
  address,
  fingerprint,
  confirm,
  replace,
  instance,
  lan,
  publicIp,
  checkedAt,
  activeAddress,
  activeFingerprint,
  standbyAddress,
  standbyFingerprint,
  activeLan,
  standbyLan,
  activePublicIp,
  standbyPublicIp,
  activeInstance,
  standbyInstance,
  activeCheckedAt,
  standbyCheckedAt,
}) {
  const slot = role === "standby" ? "standby" : role === "active" ? "active" : "";
  if (!slot) return { status: 400, error: "Say whether this server is active or on standby." };
  if (!owner) return { status: 403, error: "Only the network owner can register a server." };
  const fp = normalizeFingerprint(fingerprint);
  if (!fp) return { status: 400, error: "This server has no key fingerprint." };
  const nextAddress = String(address || "").trim().slice(0, 200);
  if (!nextAddress) return { status: 400, error: "Set the public address in the PJDesk Server window." };
  if (isPrivateAddress(nextAddress) && !confirm) {
    return {
      status: 400,
      error: "Confirm the public address in the PJDesk Server window. A local network address is not published on its own.",
    };
  }
  const id = String(instance || "").trim().slice(0, 64);
  const current = {
    activeAddress: String(activeAddress || ""),
    activeFingerprint: normalizeFingerprint(activeFingerprint),
    standbyAddress: String(standbyAddress || ""),
    standbyFingerprint: normalizeFingerprint(standbyFingerprint),
    activeLan: String(activeLan || ""),
    standbyLan: String(standbyLan || ""),
    activePublicIp: String(activePublicIp || ""),
    standbyPublicIp: String(standbyPublicIp || ""),
    activeInstance: String(activeInstance || ""),
    standbyInstance: String(standbyInstance || ""),
    activeCheckedAt: Number(activeCheckedAt) || 0,
    standbyCheckedAt: Number(standbyCheckedAt) || 0,
  };
  const next = { ...current };
  const slotAddr = current[`${slot}Address`].trim();
  const slotFp = current[`${slot}Fingerprint`];
  const slotInstance = current[`${slot}Instance`];
  const sameMachine = id !== "" && slotInstance === id;
  const differentFp = slotFp !== "" && slotFp !== fp;
  const differentAddr = slotAddr !== "" && slotAddr !== nextAddress;
  if (!replace && (differentFp || (differentAddr && !sameMachine))) {
    return {
      status: 409,
      needsConfirm: true,
      error: "My network already lists a different server. Confirm in the PJDesk Server window to replace it.",
      changed: false,
      next: current,
      changes: [],
    };
  }
  const changes = [];
  const other = slot === "active" ? "standby" : "active";
  const otherInstance = current[`${other}Instance`];
  if (id && otherInstance === id) {
    next[`${other}Address`] = "";
    next[`${other}Fingerprint`] = "";
    next[`${other}Lan`] = "";
    next[`${other}PublicIp`] = "";
    next[`${other}Instance`] = "";
    next[`${other}CheckedAt`] = 0;
    changes.push({ action: `clear-${other}`, detail: "This server changed role." });
  }
  const nextLan = lanText(lan);
  const nextIp = String(publicIp || "").trim().slice(0, 80);
  const nextChecked = Number(checkedAt) || 0;
  const unchanged =
    slotAddr === nextAddress &&
    slotFp === fp &&
    current[`${slot}Lan`] === nextLan &&
    (nextIp === "" || current[`${slot}PublicIp`] === nextIp) &&
    (id === "" || slotInstance === id) &&
    changes.length === 0;
  if (unchanged) return { status: 200, changed: false, next: current, changes: [] };
  next[`${slot}Address`] = nextAddress;
  next[`${slot}Fingerprint`] = fp;
  next[`${slot}Lan`] = nextLan;
  if (nextIp) next[`${slot}PublicIp`] = nextIp;
  if (id) next[`${slot}Instance`] = id;
  if (nextChecked) next[`${slot}CheckedAt`] = nextChecked;
  changes.unshift({ action: `register-${slot}`, detail: nextAddress });
  return { status: 200, changed: true, next, changes };
}

export function inviteError({ revoked, expiresAt, uses, maxUses, now, boundEmail, callerEmail }) {
  if (revoked) return "This network key was revoked.";
  if (expiresAt != null && now >= expiresAt) return "This network key has expired.";
  if (maxUses != null && uses >= maxUses) {
    return "This network key has already been used the maximum number of times.";
  }
  const bound = String(boundEmail || "").trim().toLowerCase();
  if (bound) {
    const caller = String(callerEmail || "").trim().toLowerCase();
    if (!caller || caller !== bound) return "This network key is for a specific account. Sign in as that account.";
  }
  return null;
}

export function decide({ requireAccounts, verified, expired, wrongServer, revoked, allowed, email }) {
  if (!verified || expired || wrongServer) {
    return requireAccounts ? "deny" : "open";
  }
  if (revoked) return "deny";
  const list = allowed || [];
  if (list.length && !list.some((e) => e.toLowerCase() === String(email || "").toLowerCase())) return "deny";
  return "signed";
}

async function readJson(request) {
  try {
    return await request.json();
  } catch {
    throw new HttpError(400, "Bad request.");
  }
}

async function sha256Hex(text) {
  return toHex(await crypto.subtle.digest("SHA-256", encoder.encode(text)));
}

// Any signed-in account. Site admin is not required and is not checked.
// Mutations call owned() and apply only to the network this account owns.
async function session(request, env) {
  const m = (request.headers.get("authorization") || "").match(/^Bearer\s+([A-Za-z0-9_-]{20,128})$/);
  if (!m) throw new HttpError(401, "Please sign in.");
  const tokenHash = await sha256Hex(m[1]);
  const row = await env.DB.prepare(
    `SELECT s.account_id, a.email FROM sessions s JOIN accounts a ON a.id = s.account_id WHERE s.token_hash = ?`,
  )
    .bind(tokenHash)
    .first();
  if (!row) throw new HttpError(401, "Please sign in.");
  return { accountId: row.account_id, email: row.email };
}

async function signingKey(env) {
  const row = await env.DB.prepare("SELECT public_key, private_pkcs8 FROM network_keys WHERE id = 1").first();
  if (row) {
    const privateKey = await crypto.subtle.importKey("pkcs8", fromHex(row.private_pkcs8), { name: "Ed25519" }, false, ["sign"]);
    return { publicKey: row.public_key, privateKey };
  }
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const pub = toHex(await crypto.subtle.exportKey("raw", pair.publicKey));
  const pkcs8 = toHex(await crypto.subtle.exportKey("pkcs8", pair.privateKey));
  await env.DB.prepare("INSERT INTO network_keys (id, public_key, private_pkcs8) VALUES (1, ?, ?)").bind(pub, pkcs8).run();
  return { publicKey: pub, privateKey: pair.privateKey };
}

function fingerprintsOf(network) {
  return [network.active_fingerprint, network.standby_fingerprint].map(normalizeFingerprint).filter(Boolean);
}

async function signToken(env, { email, network, deviceId }) {
  const exp = now() + TOKEN_TTL;
  const body = canonical({
    email,
    networkId: network.id,
    fingerprints: fingerprintsOf(network),
    deviceId,
    exp,
  });
  const { privateKey } = await signingKey(env);
  const sig = await crypto.subtle.sign("Ed25519", privateKey, encoder.encode(body));
  return { access: `${toHex(encoder.encode(body))}.${toHex(sig)}`, exp };
}

function networkView(row) {
  return {
    id: row.id,
    name: row.name || "",
    active_address: row.active_address || "",
    active_fingerprint: row.active_fingerprint || "",
    standby_address: row.standby_address || "",
    standby_fingerprint: row.standby_fingerprint || "",
    require_accounts: requireAccountsOn(row.require_accounts),
    active_lan: row.active_lan || "",
    standby_lan: row.standby_lan || "",
    active_public_ip: row.active_public_ip || "",
    standby_public_ip: row.standby_public_ip || "",
    active_ip_checked_at: row.active_ip_checked_at || 0,
    standby_ip_checked_at: row.standby_ip_checked_at || 0,
  };
}

async function owned(env, accountId) {
  return env.DB.prepare("SELECT * FROM networks WHERE owner_id = ?").bind(accountId).first();
}

async function membership(env, email) {
  return env.DB.prepare(
    `SELECT n.*, m.revoked_at AS member_revoked
       FROM network_members m JOIN networks n ON n.id = m.network_id
      WHERE m.email = ? AND m.revoked_at IS NULL`,
  )
    .bind(email)
    .first();
}

async function networkFor(env, account) {
  const own = await owned(env, account.accountId);
  if (own) return { network: own, role: "owner" };
  const member = await membership(env, account.email);
  if (member && !member.member_revoked) return { network: member, role: "member" };
  return { network: null, role: null };
}

function cleanComputer(row) {
  const id = String(row.id || row.device_id || "").replace(/\D/g, "");
  if (id.length !== 9) return null;
  const hostname = String(row.hostname || "").slice(0, 80);
  let name = String(row.name || "").slice(0, 80);
  const compact = name.replace(/\s/g, "");
  if (compact === id) name = "";
  if (hostname && name.toLowerCase() === hostname.toLowerCase()) name = "";
  const position = Number(row.position);
  const hidden = row.hidden === true || row.hidden === 1;
  return {
    id,
    name: hidden ? "" : name,
    hostname,
    os_family: String(row.os_family || row.osFamily || "").slice(0, 32),
    os_version: String(row.os_version || row.osVersion || "").slice(0, 40),
    starred: row.starred === true || row.starred === 1,
    position: Number.isFinite(position) && position > 0 ? Math.min(position, 1_000_000) : 0,
    hidden,
    display_name: String(row.display_name || "").replace(/[\u0000-\u001f\u007f]/g, "").trim().slice(0, 64),
  };
}

/** Fields the account stores for one saved computer. Passwords are not among them. */
/**
 * The PJDesk client sends a network refresh token. The PJDesk Server window
 * sends its pjdesk.com sign-in (Bearer) and no refresh token. Both read and
 * write the same sealed PIN.
 */
export function appLockAuthMode(header, body) {
  const bearer = /^Bearer\s+\S+/.test(String(header || ""));
  if (bearer && !body?.refresh) return "bearer";
  return "device";
}

/** Null when `text` is empty or a sealed PJLOCK01 blob. A string is the reason to reject it. */
export function acceptLock(text) {
  if (text == null || text === "") return null;
  if (typeof text !== "string") return "That app lock is not valid.";
  if (text.length > 8000) return "That app lock is too large.";
  let raw;
  try {
    raw = atob(text);
  } catch {
    return "That app lock is not valid.";
  }
  if (raw.length < 8 + 12 + 16) return "That app lock is not valid.";
  if (raw.slice(0, 8) !== "PJLOCK01") return "That app lock is not valid.";
  return null;
}

/** Null when `text` is empty or a sealed PJVAULT1 blob. A string is the reason to reject it. */
export function acceptVault(text) {
  if (text == null || text === "") return null;
  if (typeof text !== "string") return "That password copy is not valid.";
  if (text.length > 200000) return "That password copy is too large.";
  let raw;
  try {
    raw = atob(text);
  } catch {
    return "That password copy is not valid.";
  }
  if (raw.length < 8 + 12 + 16) return "That password copy is not valid.";
  if (raw.slice(0, 8) !== "PJVAULT1") return "That password copy is not valid.";
  return null;
}

export function savedShape(row) {
  return cleanComputer(row);
}

/** Nine digits, or "" when it is not a PJDesk ID. */
export function pjdeskId(raw) {
  const id = String(raw || "").replace(/\D/g, "");
  return /^\d{9}$/.test(id) ? id : "";
}

/** Drops rows whose id was deleted from the account. */
export function keepListed(rows, removed) {
  const gone = new Set((removed || []).map((id) => pjdeskId(id)).filter(Boolean));
  if (!gone.size) return rows || [];
  return (rows || []).filter((row) => {
    const id = pjdeskId(row?.id || row?.device_id || row?.pjdesk_id);
    return !id || !gone.has(id);
  });
}

export function ownerMayDelete(callerEmail, ownerEmail) {
  const owner = String(ownerEmail || "").trim().toLowerCase();
  const caller = String(callerEmail || "").trim().toLowerCase();
  return !!owner && caller === owner;
}

function missingSavedColumn(err) {
  const message = columnMessage(err);
  return /no such column/i.test(message) && /starred|position|hidden/i.test(message);
}

function columnMessage(err) {
  const parts = [];
  let cur = err;
  for (let i = 0; i < 4 && cur; i += 1) {
    parts.push(typeof cur === "string" ? cur : cur.message || "");
    cur = cur.cause;
  }
  return parts.join(" ");
}

function missingHostnameColumn(err) {
  const parts = [];
  let cur = err;
  for (let i = 0; i < 4 && cur; i += 1) {
    parts.push(typeof cur === "string" ? cur : cur.message || "");
    cur = cur.cause;
  }
  const message = parts.join(" ");
  return /no such column/i.test(message) && /hostname/i.test(message);
}

function missingSavedDisplay(err) {
  const message = columnMessage(err);
  return /no such column/i.test(message) && /display_name/i.test(message);
}

async function computersFor(env, networkId, accountId) {
  const rows = await computersForStored(env, networkId, accountId);
  return keepListed(rows, await removedFor(env, networkId));
}

async function computersForStored(env, networkId, accountId) {
  try {
    const { results } = await env.DB.prepare(
      `SELECT device_id, name, hostname, os_family, os_version, starred, position, hidden, display_name FROM network_computers
        WHERE network_id = ? AND account_id = ? ORDER BY position, updated_at DESC LIMIT 500`,
    )
      .bind(networkId, accountId)
      .all();
    return (results || []).map((r) => ({
      id: r.device_id,
      name: r.hidden ? "" : r.name,
      hostname: r.hostname || "",
      os_family: r.os_family,
      os_version: r.os_version,
      starred: !!r.starred,
      position: Number(r.position) || 0,
      hidden: !!r.hidden,
      display_name: r.display_name || "",
    }));
  } catch (err) {
    if (missingSavedDisplay(err)) return computersForLegacy(env, networkId, accountId);
    return computersForLegacy(env, networkId, accountId, err);
  }
}

async function computersForLegacy(env, networkId, accountId, err) {
  try {
    if (err) throw err;
    const { results } = await env.DB.prepare(
      `SELECT device_id, name, hostname, os_family, os_version, starred, position, hidden FROM network_computers
        WHERE network_id = ? AND account_id = ? ORDER BY position, updated_at DESC LIMIT 500`,
    )
      .bind(networkId, accountId)
      .all();
    return (results || []).map((r) => ({
      id: r.device_id,
      name: r.hidden ? "" : r.name,
      hostname: r.hostname || "",
      os_family: r.os_family,
      os_version: r.os_version,
      starred: !!r.starred,
      position: Number(r.position) || 0,
      hidden: !!r.hidden,
    }));
  } catch (err) {
    if (!missingSavedColumn(err) && !missingHostnameColumn(err)) throw err;
    if (missingSavedColumn(err)) {
      const { results } = await env.DB.prepare(
        `SELECT device_id, name, hostname, os_family, os_version FROM network_computers
          WHERE network_id = ? AND account_id = ? ORDER BY updated_at DESC LIMIT 500`,
      )
        .bind(networkId, accountId)
        .all();
      return (results || []).map((r) => ({
        id: r.device_id,
        name: r.name,
        hostname: r.hostname || "",
        os_family: r.os_family,
        os_version: r.os_version,
      }));
    }
    const { results } = await env.DB.prepare(
      `SELECT device_id, name, os_family, os_version FROM network_computers
        WHERE network_id = ? AND account_id = ? ORDER BY updated_at DESC LIMIT 500`,
    )
      .bind(networkId, accountId)
      .all();
    return (results || []).map((r) => ({
      id: r.device_id,
      name: r.name,
      hostname: "",
      os_family: r.os_family,
      os_version: r.os_version,
    }));
  }
}

async function saveComputers(env, networkId, accountId, list) {
  const removed = await removedFor(env, networkId);
  const rows = keepListed((Array.isArray(list) ? list : []).map(cleanComputer).filter(Boolean), removed).slice(0, 500);
  const t = now();
  if (!rows.length) return;
  try {
    await saveComputersDisplay(env, networkId, accountId, rows, t);
  } catch (err) {
    if (!missingSavedDisplay(err) && !missingSavedColumn(err) && !missingHostnameColumn(err)) throw err;
    await saveComputersLegacy(env, networkId, accountId, rows, t);
  }
}

async function saveComputersDisplay(env, networkId, accountId, rows, t) {
  const writes = rows.map((c) =>
    env.DB.prepare(
      `INSERT INTO network_computers (network_id, account_id, device_id, name, hostname, os_family, os_version, updated_at, starred, position, hidden, display_name)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT(network_id, account_id, device_id) DO UPDATE SET
         name = excluded.name, hostname = excluded.hostname, os_family = excluded.os_family,
         os_version = excluded.os_version, updated_at = excluded.updated_at,
         starred = excluded.starred, position = excluded.position, hidden = excluded.hidden,
         display_name = CASE WHEN excluded.display_name != '' THEN excluded.display_name ELSE network_computers.display_name END`,
    ).bind(
      networkId,
      accountId,
      c.id,
      c.hidden ? "" : c.name,
      c.hidden ? "" : c.hostname,
      c.os_family,
      c.os_version,
      t,
      c.starred ? 1 : 0,
      c.position || 0,
      c.hidden ? 1 : 0,
      c.display_name || "",
    ),
  );
  await env.DB.batch(writes);
}

async function saveComputersLegacy(env, networkId, accountId, rows, t) {
  const withHost = rows.map((c) =>
    env.DB.prepare(
      `INSERT INTO network_computers (network_id, account_id, device_id, name, hostname, os_family, os_version, updated_at, starred, position, hidden)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT(network_id, account_id, device_id) DO UPDATE SET
         name = excluded.name, hostname = excluded.hostname, os_family = excluded.os_family,
         os_version = excluded.os_version, updated_at = excluded.updated_at,
         starred = excluded.starred, position = excluded.position, hidden = excluded.hidden`,
    ).bind(
      networkId,
      accountId,
      c.id,
      c.hidden ? "" : c.name,
      c.hidden ? "" : c.hostname,
      c.os_family,
      c.os_version,
      t,
      c.starred ? 1 : 0,
      c.position || 0,
      c.hidden ? 1 : 0,
    ),
  );
  if (!withHost.length) return;
  try {
    await env.DB.batch(withHost);
  } catch (err) {
    if (missingSavedColumn(err)) {
      const mid = rows
        .filter((c) => !c.hidden)
        .map((c) =>
          env.DB.prepare(
            `INSERT INTO network_computers (network_id, account_id, device_id, name, hostname, os_family, os_version, updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT(network_id, account_id, device_id) DO UPDATE SET
               name = excluded.name, hostname = excluded.hostname, os_family = excluded.os_family,
               os_version = excluded.os_version, updated_at = excluded.updated_at`,
          ).bind(networkId, accountId, c.id, c.name, c.hostname, c.os_family, c.os_version, t),
        );
      if (mid.length) await env.DB.batch(mid);
      return;
    }
    if (!missingHostnameColumn(err)) throw err;
    const legacy = rows.map((c) =>
      env.DB.prepare(
        `INSERT INTO network_computers (network_id, account_id, device_id, name, os_family, os_version, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(network_id, account_id, device_id) DO UPDATE SET
           name = excluded.name, os_family = excluded.os_family, os_version = excluded.os_version, updated_at = excluded.updated_at`,
      ).bind(networkId, accountId, c.id, c.name, c.os_family, c.os_version, t),
    );
    await env.DB.batch(legacy);
  }
}

function validDevice(id) {
  return typeof id === "string" && /^[A-Za-z0-9._-]{1,80}$/.test(id);
}

export function cleanLabel(value, max) {
  return String(value || "")
    .replace(/[\u0000-\u001f\u007f]/g, "")
    .trim()
    .slice(0, max);
}

/** A later report fills blanks and never erases a hostname or user with an empty one. */
export function applyPresence(prev, next) {
  const hostname = cleanLabel(next.hostname, 64);
  const osFamily = cleanLabel(next.os_family, 32);
  const osVersion = cleanLabel(next.os_version, 40);
  const user = cleanLabel(next.console_user, 80);
  const last = Number(next.last_seen) || 0;
  return {
    hostname: hostname || prev.hostname || "",
    os_family: osFamily || prev.os_family || "",
    os_version: osVersion || prev.os_version || "",
    console_user: user || prev.console_user || "",
    display_name: Object.prototype.hasOwnProperty.call(next, "display_name")
      ? cleanLabel(next.display_name, 64)
      : prev.display_name || "",
    last_seen: Math.max(Number(prev.last_seen) || 0, last),
    online: next.online ? 1 : 0,
    signed_in: next.signed_in ? 1 : 0,
  };
}

/** The last signed-in user is visible only to the network owner. */
export function presenceView(row, role) {
  return {
    id: String(row.id || ""),
    hostname: row.hostname || "",
    os_family: row.os_family || "",
    os_version: row.os_version || "",
    console_user: role === "owner" ? row.console_user || "" : "",
    display_name: row.display_name || "",
    last_seen: Number(row.last_seen) || 0,
    online: !!row.online,
    signed_in: !!row.signed_in,
  };
}

function missingPresence(err) {
  const message = String(err?.message || err);
  return /no such table/i.test(message);
}

function missingDisplay(err) {
  const message = String(err?.message || err);
  return /no such column: display_name/i.test(message);
}

async function presenceFor(env, networkId, role) {
  const rows = await presenceForStored(env, networkId, role);
  return keepListed(rows, await removedFor(env, networkId));
}

async function presenceForStored(env, networkId, role) {
  try {
    const cutoff = now() - 90 * 24 * 3600;
    const { results } = await env.DB.prepare(
      `SELECT pjdesk_id, hostname, os_family, os_version, console_user, display_name, last_seen, online, signed_in
         FROM network_presence WHERE network_id = ? AND last_seen >= ? ORDER BY last_seen DESC LIMIT 500`,
    )
      .bind(networkId, cutoff)
      .all();
    return (results || []).map((row) =>
      presenceView(
        {
          id: row.pjdesk_id,
          hostname: row.hostname,
          os_family: row.os_family,
          os_version: row.os_version,
          console_user: row.console_user,
          display_name: row.display_name,
          last_seen: row.last_seen,
          online: row.online,
          signed_in: row.signed_in,
        },
        role,
      ),
    );
  } catch (err) {
    if (missingDisplay(err)) return presenceForLegacy(env, networkId, role);
    if (missingPresence(err)) return [];
    throw err;
  }
}

async function presenceForLegacy(env, networkId, role) {
  const cutoff = now() - 90 * 24 * 3600;
  const { results } = await env.DB.prepare(
    `SELECT pjdesk_id, hostname, os_family, os_version, console_user, last_seen, online, signed_in
       FROM network_presence WHERE network_id = ? AND last_seen >= ? ORDER BY last_seen DESC LIMIT 500`,
  )
    .bind(networkId, cutoff)
    .all();
  return (results || []).map((row) =>
    presenceView(
      {
        id: row.pjdesk_id,
        hostname: row.hostname,
        os_family: row.os_family,
        os_version: row.os_version,
        console_user: row.console_user,
        last_seen: row.last_seen,
        online: row.online,
        signed_in: row.signed_in,
      },
      role,
    ),
  );
}

async function accountRole(env, network, account) {
  if (network.owner_id && account.accountId && network.owner_id === account.accountId) return "owner";
  if (!network.owner_id) return "member";
  const owner = await env.DB.prepare("SELECT email FROM accounts WHERE id = ?").bind(network.owner_id).first();
  return owner?.email === account.email ? "owner" : "member";
}

function missingRemoved(err) {
  const message = columnMessage(err);
  return /no such table/i.test(message) && /network_removed/i.test(message);
}

function missingPjdeskColumn(err) {
  const message = columnMessage(err);
  return /no such column/i.test(message) && /pjdesk_id/i.test(message);
}

async function removedFor(env, networkId) {
  try {
    const { results } = await env.DB.prepare("SELECT pjdesk_id FROM network_removed WHERE network_id = ?")
      .bind(networkId)
      .all();
    return (results || []).map((row) => row.pjdesk_id).filter(Boolean);
  } catch (err) {
    if (missingRemoved(err)) return [];
    throw err;
  }
}

async function clearRemoved(env, networkId, id) {
  try {
    await env.DB.prepare("DELETE FROM network_removed WHERE network_id = ? AND pjdesk_id = ?").bind(networkId, id).run();
  } catch (err) {
    if (!missingRemoved(err)) throw err;
  }
}

async function rememberPjdesk(env, networkId, deviceId, pjdesk, hardwareId) {
  if (!pjdesk || !deviceId) return;
  const hardware = String(hardwareId || "").trim().slice(0, 64);
  let previous = "";
  try {
    const current = await env.DB.prepare(
      "SELECT pjdesk_id FROM network_devices WHERE network_id = ? AND device_id = ?",
    )
      .bind(networkId, deviceId)
      .first();
    previous = current?.pjdesk_id || "";
  } catch (err) {
    if (!missingPjdeskColumn(err)) throw err;
  }
  if (previous && previous !== pjdesk) await dropStalePresence(env, networkId, previous);
  if (hardware) await dropSameHardware(env, networkId, hardware, pjdesk);
  if (hardware) {
    try {
      await env.DB.prepare(
        "UPDATE network_presence SET hardware_id = ? WHERE network_id = ? AND pjdesk_id = ?",
      )
        .bind(hardware, networkId, pjdesk)
        .run();
    } catch (err) {
      if (!missingHardwareColumn(err) && !missingPresence(err)) throw err;
    }
  }
  try {
    if (hardware) {
      await env.DB.prepare(
        "UPDATE network_devices SET pjdesk_id = ?, hardware_id = ? WHERE network_id = ? AND device_id = ?",
      )
        .bind(pjdesk, hardware, networkId, deviceId)
        .run();
    } else {
      await env.DB.prepare(
        "UPDATE network_devices SET pjdesk_id = ? WHERE network_id = ? AND device_id = ? AND pjdesk_id != ?",
      )
        .bind(pjdesk, networkId, deviceId, pjdesk)
        .run();
    }
  } catch (err) {
    if (missingHardwareColumn(err) && hardware) {
      try {
        await env.DB.prepare(
          "UPDATE network_devices SET pjdesk_id = ? WHERE network_id = ? AND device_id = ? AND pjdesk_id != ?",
        )
          .bind(pjdesk, networkId, deviceId, pjdesk)
          .run();
      } catch (again) {
        if (!missingPjdeskColumn(again)) throw again;
      }
    } else if (!missingPjdeskColumn(err)) throw err;
  }
}

function missingHardwareColumn(err) {
  return /no such column/i.test(String(err?.message || err)) && /hardware_id/i.test(String(err?.message || err));
}

/** Drops an old PJDesk id from the list without deleting this device's sign-in. */
async function dropStalePresence(env, networkId, oldId) {
  if (!/^\d{9}$/.test(oldId)) return;
  try {
    await env.DB.prepare(
      `INSERT INTO network_removed (network_id, pjdesk_id, removed_at) VALUES (?, ?, ?)
       ON CONFLICT(network_id, pjdesk_id) DO UPDATE SET removed_at = excluded.removed_at`,
    )
      .bind(networkId, oldId, now())
      .run();
  } catch (err) {
    if (!missingRemoved(err)) throw err;
  }
  await env.DB.prepare("DELETE FROM network_computers WHERE network_id = ? AND device_id = ?").bind(networkId, oldId).run();
  try {
    await env.DB.prepare("DELETE FROM network_presence WHERE network_id = ? AND pjdesk_id = ?").bind(networkId, oldId).run();
  } catch (err) {
    if (!missingPresence(err)) throw err;
  }
}

async function dropSameHostname(env, networkId, hostname, keepId) {
  try {
    const { results } = await env.DB.prepare(
      `SELECT pjdesk_id FROM network_presence
        WHERE network_id = ? AND lower(hostname) = lower(?) AND pjdesk_id != ? AND online = 0`,
    )
      .bind(networkId, hostname, keepId)
      .all();
    for (const row of results || []) {
      if (row?.pjdesk_id) await dropStalePresence(env, networkId, row.pjdesk_id);
    }
  } catch (err) {
    if (!missingPresence(err)) throw err;
  }
}

async function dropSameHardware(env, networkId, hardware, keepId) {
  try {
    const { results } = await env.DB.prepare(
      "SELECT pjdesk_id FROM network_presence WHERE network_id = ? AND hardware_id = ? AND pjdesk_id != ?",
    )
      .bind(networkId, hardware, keepId)
      .all();
    for (const row of results || []) {
      if (row?.pjdesk_id) await dropStalePresence(env, networkId, row.pjdesk_id);
    }
  } catch (err) {
    if (!missingHardwareColumn(err) && !missingPresence(err)) throw err;
  }
}

/** Deletes the saved rows, the presence slot, and a sign-in tied to this ID. */
export async function forgetComputer(env, networkId, rawId) {
  const id = pjdeskId(rawId);
  if (!id) throw new HttpError(400, "That is not a PJDesk ID.");
  try {
    await env.DB.prepare(
      `INSERT INTO network_removed (network_id, pjdesk_id, removed_at) VALUES (?, ?, ?)
       ON CONFLICT(network_id, pjdesk_id) DO UPDATE SET removed_at = excluded.removed_at`,
    )
      .bind(networkId, id, now())
      .run();
  } catch (err) {
    if (missingRemoved(err)) {
      throw new HttpError(
        503,
        "Update the account service before deleting a computer. On your PC, in cloud\\, run cloud\\deploy.cmd.",
      );
    }
    throw err;
  }
  await env.DB.prepare("DELETE FROM network_computers WHERE network_id = ? AND device_id = ?").bind(networkId, id).run();
  try {
    await env.DB.prepare("DELETE FROM network_presence WHERE network_id = ? AND pjdesk_id = ?").bind(networkId, id).run();
  } catch (err) {
    if (!missingPresence(err)) throw err;
  }
  await env.DB.prepare("DELETE FROM network_devices WHERE network_id = ? AND device_id = ?").bind(networkId, id).run();
  try {
    await env.DB.prepare("DELETE FROM network_devices WHERE network_id = ? AND pjdesk_id = ?").bind(networkId, id).run();
  } catch (err) {
    if (!missingPjdeskColumn(err)) throw err;
  }
  return id;
}

async function issue(env, account, network, deviceId, deviceName, computers) {
  if (!validDevice(deviceId)) throw new HttpError(400, "This device needs an id.");
  if (!network.active_address) throw new HttpError(400, NO_SERVER_MESSAGE);
  const { access, exp } = await signToken(env, { email: account.email, network, deviceId });
  const refresh = randomHex(32);
  const refreshHash = await sha256Hex(refresh);
  await env.DB.prepare(
    `INSERT INTO network_devices (network_id, device_id, account_id, email, name, refresh_hash, enrolled_at, revoked_at)
     VALUES (?, ?, ?, ?, ?, ?, ?, NULL)
     ON CONFLICT(network_id, device_id) DO UPDATE SET
       account_id = excluded.account_id, email = excluded.email, name = excluded.name,
       refresh_hash = excluded.refresh_hash, revoked_at = NULL`,
  )
    .bind(network.id, deviceId, account.accountId, account.email, String(deviceName || "").slice(0, 80), refreshHash, now())
    .run();
  if (computers) await saveComputers(env, network.id, account.accountId, computers);
  const saved = await computersFor(env, network.id, account.accountId);
  const role = await accountRole(env, network, account);
  const presence = await presenceFor(env, network.id, role);
  return {
    access,
    exp,
    refresh,
    device_id: deviceId,
    network: networkView(network),
    computers: saved,
    presence,
    removed: await removedFor(env, network.id),
  };
}

export async function route(request, env, url) {
  try {
    return await handle(request, env, url);
  } catch (e) {
    if (e instanceof HttpError) {
      return json(
        { error: e.message, code: e.code || "", role: e.role || "", needs_confirm: !!e.needsConfirm },
        e.status,
      );
    }
    throw e;
  }
}

async function handle(request, env, url) {
  const p = url.pathname;
  const m = request.method;
  if (p === "/api/support/offer" && m === "POST") return supportOffer(request, env);
  if (p === "/api/support/revoke" && m === "POST") return supportRevoke(request, env);
  if (p === "/api/support/lookup" && m === "POST") return supportLookup(request, env);
  if (p === "/api/network/key" && m === "GET") {
    const { publicKey } = await signingKey(env);
    return json({ public_key: publicKey });
  }
  if (!p.startsWith("/api/network")) return null;

  if (p === "/api/network/enroll" && m === "POST") return enroll(request, env);
  if (p === "/api/network/refresh" && m === "POST") return refresh(request, env);
  if (p === "/api/network/passwords" && m === "POST") return passwordVault(request, env);
  if (p === "/api/network/app-lock" && m === "POST") return appLock(request, env);
  if (p === "/api/network/redeem" && m === "POST") return redeem(request, env);
  if (p === "/api/network/computers/delete" && m === "POST") return deleteComputer(request, env);

  const account = await session(request, env);

  if (p === "/api/network" && m === "GET") return getNetwork(env, account);
  if (p === "/api/network" && m === "PUT") return putNetwork(request, env, account);
  if (p === "/api/network/register" && m === "POST") return registerServer(request, env, account);
  if (p === "/api/network/members" && m === "POST") return addMember(request, env, account);
  if (p === "/api/network/members" && m === "DELETE") return removeMember(request, env, account);
  if (p === "/api/network/revoke-device" && m === "POST") return revokeDevice(request, env, account);
  if (p === "/api/network/token" && m === "POST") return token(request, env, account);
  if (p === "/api/network/computers" && m === "GET") return listComputers(env, account);
  if (p === "/api/network/computers" && m === "PUT") return putComputers(request, env, account);
  if (p === "/api/network/presence" && m === "POST") return postPresence(request, env, account);
  if (p === "/api/network/enroll-code" && m === "POST") return enrollCode(env, account);
  if (p === "/api/network/join-code" && m === "GET") return listJoinCodes(env, account);
  if (p === "/api/network/join-code" && m === "POST") return createJoinCode(env, account);
  if (p === "/api/network/join-code/revoke" && m === "POST") return revokeJoinCode(request, env, account);
  if (p === "/api/network/invites" && m === "POST") return createInvite(request, env, account);
  if (p === "/api/network/invites/revoke" && m === "POST") return revokeInvite(request, env, account);
  if (p === "/api/network/policy" && m === "GET") return policy(env, account);
  return null;
}

async function getNetwork(env, account) {
  const { network, role } = await networkFor(env, account);
  if (!network) return json({ network: null, role: null, computers: [] });
  const computers = await computersFor(env, network.id, account.accountId);
  const body = { role, network: networkView(network), computers };
  if (role === "owner") {
    const members = await env.DB.prepare(
      "SELECT email, created_at, revoked_at FROM network_members WHERE network_id = ? ORDER BY email",
    )
      .bind(network.id)
      .all();
    const devices = await env.DB.prepare(
      "SELECT device_id, email, name, enrolled_at, revoked_at FROM network_devices WHERE network_id = ? ORDER BY enrolled_at DESC",
    )
      .bind(network.id)
      .all();
    body.members = members.results || [];
    body.devices = devices.results || [];
    body.invites = await invitesFor(env, network.id);
    body.changes = await changesFor(env, network.id);
    body.presence = await presenceFor(env, network.id, role);
    body.removed = await removedFor(env, network.id);
    body.note = "Computers that have not signed in are listed in the PJDesk Server window until you turn on Require signed-in accounts.";
  }
  return json(body);
}

async function putNetwork(request, env, account) {
  const body = await readJson(request);
  let row = await owned(env, account.accountId);
  if (!row) {
    const member = await membership(env, account.email);
    if (member) throw new HttpError(403, "Only the network owner can change this network.");
    const id = `net_${randomHex(8)}`;
    await env.DB.prepare(
      `INSERT INTO networks (id, owner_id, name, active_address, active_fingerprint, standby_address, standby_fingerprint, require_accounts, created_at)
       VALUES (?, ?, '', '', '', '', '', 0, ?)`,
    )
      .bind(id, account.accountId, now())
      .run();
    row = await owned(env, account.accountId);
  }
  const next = {
    name: body.name != null ? String(body.name).slice(0, 80) : row.name,
    active_address: body.active_address != null ? String(body.active_address).trim().slice(0, 200) : row.active_address,
    active_fingerprint:
      body.active_fingerprint != null ? normalizeFingerprint(body.active_fingerprint).slice(0, 64) : row.active_fingerprint,
    standby_address: body.standby_address != null ? String(body.standby_address).trim().slice(0, 200) : row.standby_address,
    standby_fingerprint:
      body.standby_fingerprint != null ? normalizeFingerprint(body.standby_fingerprint).slice(0, 64) : row.standby_fingerprint,
    require_accounts: body.require_accounts === true || body.require_accounts === 1
      ? 1
      : body.require_accounts === false || body.require_accounts === 0
        ? 0
        : requireAccountsOn(row.require_accounts) ? 1 : 0,
  };
  await env.DB.prepare(
    `UPDATE networks SET name = ?, active_address = ?, active_fingerprint = ?, standby_address = ?, standby_fingerprint = ?, require_accounts = ?
      WHERE id = ?`,
  )
    .bind(next.name, next.active_address, next.active_fingerprint, next.standby_address, next.standby_fingerprint, next.require_accounts, row.id)
    .run();
  const edited = [];
  if (next.active_address !== row.active_address || next.active_fingerprint !== row.active_fingerprint) edited.push("active server");
  if (next.standby_address !== row.standby_address || next.standby_fingerprint !== row.standby_fingerprint) edited.push("standby server");
  if (next.require_accounts !== row.require_accounts) edited.push("signed-in accounts");
  if (edited.length) await logChange(env, row.id, account.email, "edit", edited.join(", "));
  return getNetwork(env, account);
}

async function registerServer(request, env, account) {
  const body = await readJson(request);
  let row = await owned(env, account.accountId);
  if (!row) {
    const member = await membership(env, account.email);
    if (member) throw new HttpError(403, "Only the network owner can register a server.");
    const id = `net_${randomHex(8)}`;
    await env.DB.prepare(
      `INSERT INTO networks (id, owner_id, name, active_address, active_fingerprint, standby_address, standby_fingerprint, require_accounts, created_at)
       VALUES (?, ?, '', '', '', '', '', 0, ?)`,
    )
      .bind(id, account.accountId, now())
      .run();
    row = await owned(env, account.accountId);
  }
  const plan = planRegistration({
    owner: true,
    role: body.role,
    address: body.address,
    fingerprint: body.fingerprint,
    confirm: !!body.confirm,
    replace: !!body.replace,
    instance: body.instance,
    lan: body.lan,
    publicIp: body.public_ip,
    checkedAt: body.checked_at,
    activeAddress: row.active_address,
    activeFingerprint: row.active_fingerprint,
    standbyAddress: row.standby_address,
    standbyFingerprint: row.standby_fingerprint,
    activeLan: row.active_lan,
    standbyLan: row.standby_lan,
    activePublicIp: row.active_public_ip,
    standbyPublicIp: row.standby_public_ip,
    activeInstance: row.active_instance,
    standbyInstance: row.standby_instance,
    activeCheckedAt: row.active_ip_checked_at,
    standbyCheckedAt: row.standby_ip_checked_at,
  });
  if (plan.changed) {
    await writeSlots(env, row, plan.next);
    for (const change of plan.changes || []) await logChange(env, row.id, account.email, change.action, change.detail);
  }
  if (plan.status !== 200) {
    const err = new HttpError(plan.status, plan.error);
    err.needsConfirm = !!plan.needsConfirm;
    throw err;
  }
  const fresh = await owned(env, account.accountId);
  return json({
    ok: true,
    changed: !!plan.changed,
    needs_confirm: !!plan.needsConfirm,
    error: plan.needsConfirm ? plan.error : "",
    network: networkView(fresh),
  });
}

async function addMember(request, env, account) {
  const row = await owned(env, account.accountId);
  if (!row) throw new HttpError(404, "Set your network before inviting someone.");
  const { email: raw } = await readJson(request);
  const email = normEmail(raw);
  if (!validEmail(email)) throw new HttpError(400, "Enter a valid email address.");
  if (email === account.email) throw new HttpError(400, "You already have access as the owner.");
  const existing = await env.DB.prepare("SELECT id FROM accounts WHERE email = ?").bind(email).first();
  await env.DB.prepare(
    `INSERT INTO network_members (network_id, email, account_id, created_at, revoked_at)
     VALUES (?, ?, ?, ?, NULL)
     ON CONFLICT(network_id, email) DO UPDATE SET account_id = excluded.account_id, revoked_at = NULL`,
  )
    .bind(row.id, email, existing?.id || null, now())
    .run();
  return getNetwork(env, account);
}

async function removeMember(request, env, account) {
  const row = await owned(env, account.accountId);
  if (!row) throw new HttpError(404, "No network.");
  const { email: raw } = await readJson(request);
  const email = normEmail(raw);
  await env.DB.prepare("UPDATE network_members SET revoked_at = ? WHERE network_id = ? AND email = ?")
    .bind(now(), row.id, email)
    .run();
  await env.DB.prepare("UPDATE network_devices SET revoked_at = ? WHERE network_id = ? AND email = ? AND revoked_at IS NULL")
    .bind(now(), row.id, email)
    .run();
  return getNetwork(env, account);
}

async function revokeDevice(request, env, account) {
  const row = await owned(env, account.accountId);
  if (!row) throw new HttpError(404, "No network.");
  const { device_id } = await readJson(request);
  if (!validDevice(device_id)) throw new HttpError(400, "Unknown device.");
  await env.DB.prepare(
    "UPDATE network_devices SET revoked_at = ?, refresh_hash = '' WHERE network_id = ? AND device_id = ?",
  )
    .bind(now(), row.id, device_id)
    .run();
  return getNetwork(env, account);
}

async function token(request, env, account) {
  const body = await readJson(request);
  const found = await networkFor(env, account);
  if (!found.network || !found.network.active_address) {
    const err = new HttpError(400, NO_SERVER_MESSAGE);
    err.code = "no_server";
    err.role = found.role || "owner";
    throw err;
  }
  return json(await issue(env, account, found.network, String(body.device_id || ""), body.device_name, body.computers));
}

async function logChange(env, networkId, actor, action, detail) {
  try {
    await env.DB.prepare(
      "INSERT INTO network_changes (network_id, at, actor, action, detail) VALUES (?, ?, ?, ?, ?)",
    )
      .bind(networkId, now(), String(actor || "").slice(0, 254), String(action || "").slice(0, 40), String(detail || "").slice(0, 300))
      .run();
  } catch (err) {
    const message = String(err?.message || err);
    if (!/no such table/i.test(message)) throw err;
  }
}

async function changesFor(env, networkId) {
  try {
    const { results } = await env.DB.prepare(
      "SELECT at, actor, action, detail FROM network_changes WHERE network_id = ? ORDER BY id DESC LIMIT 20",
    )
      .bind(networkId)
      .all();
    return results || [];
  } catch (err) {
    const message = String(err?.message || err);
    if (/no such table/i.test(message)) return [];
    throw err;
  }
}

async function writeSlots(env, row, next) {
  await env.DB.prepare(
    `UPDATE networks SET active_address = ?, active_fingerprint = ?, standby_address = ?, standby_fingerprint = ?,
        active_lan = ?, standby_lan = ?, active_public_ip = ?, standby_public_ip = ?,
        active_instance = ?, standby_instance = ?, active_ip_checked_at = ?, standby_ip_checked_at = ?
      WHERE id = ?`,
  )
    .bind(
      next.activeAddress,
      next.activeFingerprint,
      next.standbyAddress,
      next.standbyFingerprint,
      next.activeLan || "",
      next.standbyLan || "",
      next.activePublicIp || "",
      next.standbyPublicIp || "",
      next.activeInstance || "",
      next.standbyInstance || "",
      next.activeCheckedAt || 0,
      next.standbyCheckedAt || 0,
      row.id,
    )
    .run();
}

async function openedDevice(env, body) {
  const refreshToken = String(body.refresh || "");
  const deviceId = String(body.device_id || "");
  if (!validDevice(deviceId) || !/^[0-9a-f]{64}$/.test(refreshToken)) throw new HttpError(401, "Sign in required.");
  const hash = await sha256Hex(refreshToken);
  const row = await env.DB.prepare(
    `SELECT d.network_id, d.account_id, d.email, d.revoked_at, d.enrolled_at
       FROM network_devices d WHERE d.device_id = ? AND d.refresh_hash = ?`,
  )
    .bind(deviceId, hash)
    .first();
  if (!row || row.revoked_at) throw new HttpError(401, "Sign in required.");
  if (now() - row.enrolled_at > REFRESH_TTL) throw new HttpError(401, "Sign in required.");
  const network = await env.DB.prepare("SELECT owner_id FROM networks WHERE id = ?").bind(row.network_id).first();
  if (!network) throw new HttpError(401, "Sign in required.");
  const owner = await env.DB.prepare("SELECT email FROM accounts WHERE id = ?").bind(network.owner_id).first();
  const member = await env.DB.prepare(
    "SELECT revoked_at FROM network_members WHERE network_id = ? AND email = ?",
  )
    .bind(row.network_id, row.email)
    .first();
  if (member?.revoked_at) throw new HttpError(401, "Sign in required.");
  if (owner?.email !== row.email && !member) throw new HttpError(401, "Sign in required.");
  return { networkId: row.network_id, accountId: row.account_id, email: row.email };
}

async function passwordVault(request, env) {
  const body = await readJson(request);
  const device = await openedDevice(env, body);
  if (Object.prototype.hasOwnProperty.call(body, "vault")) {
    const vault = String(body.vault ?? "");
    const problem = acceptVault(vault);
    if (problem) throw new HttpError(400, problem);
    const updated = now();
    try {
      await env.DB.prepare(
        `INSERT INTO network_password_vault (network_id, account_id, ciphertext, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(network_id, account_id) DO UPDATE SET
           ciphertext = excluded.ciphertext, updated_at = excluded.updated_at`,
      )
        .bind(device.networkId, device.accountId, vault, updated)
        .run();
    } catch (err) {
      if (/no such table/i.test(String(err?.message || err))) {
        throw new HttpError(503, "Password sync is not ready yet. Run cloud\\deploy.cmd.");
      }
      throw err;
    }
    return json({ ok: true, updated_at: updated });
  }
  try {
    const row = await env.DB.prepare(
      "SELECT ciphertext, updated_at FROM network_password_vault WHERE network_id = ? AND account_id = ?",
    )
      .bind(device.networkId, device.accountId)
      .first();
    return json({ vault: row?.ciphertext || "", updated_at: row?.updated_at || 0 });
  } catch (err) {
    if (/no such table/i.test(String(err?.message || err))) return json({ vault: "", updated_at: 0 });
    throw err;
  }
}

async function appLock(request, env) {
  const body = await readJson(request);
  const header = request.headers.get("authorization") || "";
  let device;
  if (appLockAuthMode(header, body) === "bearer") {
    const account = await session(request, env);
    const found = await networkFor(env, account);
    if (!found.network) throw new HttpError(403, "This account is not on a network.");
    device = { networkId: found.network.id, accountId: account.accountId, email: account.email };
  } else {
    device = await openedDevice(env, body);
  }
  if (Object.prototype.hasOwnProperty.call(body, "lock")) {
    const lock = String(body.lock ?? "");
    const problem = acceptLock(lock);
    if (problem) throw new HttpError(400, problem);
    const updated = now();
    try {
      await env.DB.prepare(
        `INSERT INTO network_app_lock (network_id, account_id, ciphertext, updated_at)
         VALUES (?, ?, ?, ?)
         ON CONFLICT(network_id, account_id) DO UPDATE SET
           ciphertext = excluded.ciphertext, updated_at = excluded.updated_at`,
      )
        .bind(device.networkId, device.accountId, lock, updated)
        .run();
    } catch (err) {
      if (/no such table/i.test(String(err?.message || err))) {
        throw new HttpError(503, "App lock is not ready yet. Run cloud\\deploy.cmd.");
      }
      throw err;
    }
    return json({ ok: true, updated_at: updated });
  }
  try {
    const row = await env.DB.prepare(
      "SELECT ciphertext, updated_at FROM network_app_lock WHERE network_id = ? AND account_id = ?",
    )
      .bind(device.networkId, device.accountId)
      .first();
    return json({ lock: row?.ciphertext || "", updated_at: row?.updated_at || 0 });
  } catch (err) {
    if (/no such table/i.test(String(err?.message || err))) return json({ lock: "", updated_at: 0 });
    throw err;
  }
}

async function refresh(request, env) {
  const body = await readJson(request);
  const refreshToken = String(body.refresh || "");
  const deviceId = String(body.device_id || "");
  if (!validDevice(deviceId) || !/^[0-9a-f]{64}$/.test(refreshToken)) throw new HttpError(401, "Sign in required.");
  const hash = await sha256Hex(refreshToken);
  const row = await env.DB.prepare(
    `SELECT d.network_id, d.account_id, d.email, d.revoked_at, d.enrolled_at, n.*
       FROM network_devices d JOIN networks n ON n.id = d.network_id
      WHERE d.device_id = ? AND d.refresh_hash = ?`,
  )
    .bind(deviceId, hash)
    .first();
  if (!row) throw new HttpError(401, "Sign in required.");
  if (row.revoked_at) {
    const viaKey = await env.DB.prepare(
      `SELECT 1 AS hit FROM network_invite_uses u
         JOIN network_invites i ON i.id = u.invite_id
        WHERE u.device_id = ? AND i.network_id = ? AND i.revoked_at IS NOT NULL LIMIT 1`,
    )
      .bind(deviceId, row.network_id)
      .first();
    throw new HttpError(401, viaKey ? "This network key was revoked." : "Sign in required.");
  }
  if (now() - row.enrolled_at > REFRESH_TTL) throw new HttpError(401, "Sign in required.");
  const member = await env.DB.prepare(
    "SELECT revoked_at FROM network_members WHERE network_id = ? AND email = ?",
  )
    .bind(row.network_id, row.email)
    .first();
  const owner = await env.DB.prepare("SELECT email FROM accounts WHERE id = ?").bind(row.owner_id).first();
  if (member?.revoked_at) throw new HttpError(401, "Sign in required.");
  if (owner?.email !== row.email && !member) throw new HttpError(401, "Sign in required.");
  const account = { accountId: row.account_id, email: row.email };
  if (requireAccountsOn(row.require_accounts) && !row.active_address) throw new HttpError(401, "Sign in required.");
  const self = pjdeskId(body.pjdesk_id);
  const hardware = String(body.hardware_id || "").trim().slice(0, 64);
  if (self) await rememberPjdesk(env, row.id, deviceId, self, hardware);
  const reportedName = String(body.hostname || "").trim().slice(0, 64);
  if (self && reportedName) await dropSameHostname(env, row.id, reportedName, self);
  if (body.reinstate === true && self) await clearRemoved(env, row.id, self);
  const { access, exp } = await signToken(env, { email: account.email, network: row, deviceId });
  if (body.computers) await saveComputers(env, row.id, account.accountId, body.computers);
  const saved = await computersFor(env, row.id, account.accountId);
  const role = owner?.email === row.email ? "owner" : "member";
  const presence = await presenceFor(env, row.id, role);
  return json({
    access,
    exp,
    refresh: refreshToken,
    device_id: deviceId,
    network: networkView(row),
    computers: saved,
    presence,
    removed: await removedFor(env, row.id),
  });
}

async function postPresence(request, env, account) {
  const found = await networkFor(env, account);
  if (!found.network || found.role !== "owner") {
    throw new HttpError(403, "Only the network owner can publish computers.");
  }
  const body = await readJson(request);
  const devices = Array.isArray(body.devices) ? body.devices.slice(0, 500) : [];
  const removed = new Set(await removedFor(env, found.network.id));
  const cutoff = now() - 90 * 24 * 3600;
  const stmts = [];
  for (const raw of devices) {
    const id = String(raw.id || "").replace(/\D/g, "");
    if (!/^\d{9}$/.test(id) || removed.has(id)) continue;
    const last = Number(raw.last_seen) || now();
    if (last < cutoff) continue;
    const next = applyPresence({}, raw);
    next.last_seen = last;
    stmts.push(
      env.DB.prepare(
        `INSERT INTO network_presence
           (network_id, pjdesk_id, hostname, os_family, os_version, console_user, display_name, last_seen, online, signed_in)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(network_id, pjdesk_id) DO UPDATE SET
           hostname = CASE WHEN excluded.hostname != '' THEN excluded.hostname ELSE network_presence.hostname END,
           os_family = CASE WHEN excluded.os_family != '' THEN excluded.os_family ELSE network_presence.os_family END,
           os_version = CASE WHEN excluded.os_version != '' THEN excluded.os_version ELSE network_presence.os_version END,
           console_user = CASE WHEN excluded.console_user != '' THEN excluded.console_user ELSE network_presence.console_user END,
           display_name = excluded.display_name,
           last_seen = CASE WHEN excluded.last_seen > network_presence.last_seen THEN excluded.last_seen ELSE network_presence.last_seen END,
           online = excluded.online,
           signed_in = excluded.signed_in`,
      ).bind(
        found.network.id,
        id,
        next.hostname,
        next.os_family,
        next.os_version,
        next.console_user,
        next.display_name,
        next.last_seen,
        next.online,
        next.signed_in,
      ),
    );
  }
  if (!stmts.length) return json({ ok: true, count: 0 });
  try {
    await env.DB.batch(stmts);
  } catch (err) {
    if (missingDisplay(err) || missingPresence(err)) return json({ ok: false, count: 0 });
    throw err;
  }
  return json({ ok: true, count: stmts.length });
}

async function listComputers(env, account) {
  const found = await networkFor(env, account);
  if (!found.network) return json({ computers: [], removed: [] });
  return json({
    computers: await computersFor(env, found.network.id, account.accountId),
    removed: await removedFor(env, found.network.id),
  });
}

async function ownerByRefresh(env, body) {
  const refreshToken = String(body.refresh || "");
  const deviceId = String(body.device_id || "");
  if (!validDevice(deviceId) || !/^[0-9a-f]{64}$/.test(refreshToken)) throw new HttpError(401, "Sign in required.");
  const hash = await sha256Hex(refreshToken);
  const row = await env.DB.prepare(
    `SELECT d.account_id, d.email, d.revoked_at, n.id AS network_id, n.owner_id
       FROM network_devices d JOIN networks n ON n.id = d.network_id
      WHERE d.device_id = ? AND d.refresh_hash = ?`,
  )
    .bind(deviceId, hash)
    .first();
  if (!row || row.revoked_at) throw new HttpError(401, "Sign in required.");
  const owner = await env.DB.prepare("SELECT email FROM accounts WHERE id = ?").bind(row.owner_id).first();
  if (!ownerMayDelete(row.email, owner?.email)) {
    throw new HttpError(403, "Only the network owner can delete a computer from the account.");
  }
  return { networkId: row.network_id, email: row.email };
}

async function deleteComputer(request, env) {
  const body = await readJson(request);
  const id = pjdeskId(body.id);
  if (!id) throw new HttpError(400, "That is not a PJDesk ID.");
  let networkId;
  let actor;
  const header = request.headers.get("authorization") || "";
  if (/^Bearer\s+\S+/.test(header) && !body.refresh) {
    const account = await session(request, env);
    const found = await networkFor(env, account);
    if (!found.network || found.role !== "owner") {
      throw new HttpError(403, "Only the network owner can delete a computer from the account.");
    }
    networkId = found.network.id;
    actor = account.email;
  } else {
    const opened = await ownerByRefresh(env, body);
    networkId = opened.networkId;
    actor = opened.email;
  }
  await forgetComputer(env, networkId, id);
  await logChange(env, networkId, actor, "delete-computer", id);
  return json({ ok: true, id, removed: await removedFor(env, networkId) });
}

async function putComputers(request, env, account) {
  const found = await networkFor(env, account);
  if (!found.network) throw new HttpError(403, "This account is not on a network.");
  const body = await readJson(request);
  await saveComputers(env, found.network.id, account.accountId, body.computers);
  return json({
    computers: await computersFor(env, found.network.id, account.accountId),
    removed: await removedFor(env, found.network.id),
  });
}

async function enrollCode(env, account) {
  const row = await owned(env, account.accountId);
  if (!row) throw new HttpError(404, "Set your network before creating a code.");
  const alphabet = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
  const bytes = crypto.getRandomValues(new Uint8Array(8));
  let code = "";
  for (const b of bytes) code += alphabet[b % alphabet.length];
  const shown = `${code.slice(0, 4)}-${code.slice(4)}`;
  const expires = now() + ENROLL_TTL;
  await env.DB.prepare("INSERT INTO enroll_codes (code_hash, network_id, created_at, expires_at) VALUES (?, ?, ?, ?)")
    .bind(await sha256Hex(code), row.id, now(), expires)
    .run();
  return json({ code: shown, expires_at: expires });
}

async function enroll(request, env) {
  const body = await readJson(request);
  const code = String(body.code || "").toUpperCase().replace(/[^A-Z0-9]/g, "");
  const deviceId = String(body.device_id || "");
  if (code.length !== 8) throw new HttpError(400, "That enrollment code is not valid.");
  await assertGuessOpen(env, request, "join");
  const hash = await sha256Hex(code);
  if (/^\d{8}$/.test(code)) {
    const joined = await redeemJoinCode(env, request, hash, deviceId, body.device_name || "Joined computer");
    if (joined) return joined;
  }
  const row = await env.DB.prepare(
    `SELECT c.expires_at, c.used_at, n.* FROM enroll_codes c JOIN networks n ON n.id = c.network_id WHERE c.code_hash = ?`,
  )
    .bind(hash)
    .first();
  if (!row || row.used_at || row.expires_at < now()) {
    await recordGuess(env, request, "join", false);
    throw new HttpError(400, "That enrollment code is not valid.");
  }
  const owner = await env.DB.prepare("SELECT id, email FROM accounts WHERE id = ?").bind(row.owner_id).first();
  if (!owner) {
    await recordGuess(env, request, "join", false);
    throw new HttpError(400, "That enrollment code is not valid.");
  }
  await env.DB.prepare("UPDATE enroll_codes SET used_at = ? WHERE code_hash = ?").bind(now(), hash).run();
  await recordGuess(env, request, "join", true);
  return json(await issue(env, { accountId: owner.id, email: owner.email }, row, deviceId, body.device_name || "Enrolled device", null));
}

function callerSubject(request) {
  const raw = request.headers.get("cf-connecting-ip") || request.headers.get("x-forwarded-for") || "unknown";
  return raw.split(",")[0].trim().slice(0, 80);
}

async function guessRow(env, request, kind) {
  const subject = await sha256Hex(`${kind}\n${callerSubject(request)}`);
  const row = await env.DB.prepare(
    "SELECT failures, window_start, locked_until FROM code_attempts WHERE subject = ? AND kind = ?",
  )
    .bind(subject, kind)
    .first();
  return { subject, row };
}

async function assertGuessOpen(env, request, kind) {
  const { row } = await guessRow(env, request, kind);
  const decision = nextGuess({
    failures: row?.failures,
    windowStart: row?.window_start,
    lockedUntil: row?.locked_until,
    now: now(),
  });
  if (decision.locked) throw new HttpError(429, decision.message);
}

async function recordGuess(env, request, kind, success, opts) {
  const { subject, row } = await guessRow(env, request, kind);
  const decision = nextGuess(
    {
      failures: row?.failures,
      windowStart: row?.window_start,
      lockedUntil: row?.locked_until,
      now: now(),
      success,
      record: !success,
    },
    opts,
  );
  await env.DB.prepare(
    `INSERT INTO code_attempts (subject, kind, failures, window_start, locked_until)
     VALUES (?, ?, ?, ?, ?)
     ON CONFLICT(subject, kind) DO UPDATE SET
       failures = excluded.failures,
       window_start = excluded.window_start,
       locked_until = excluded.locked_until`,
  )
    .bind(subject, kind, decision.failures, decision.windowStart, decision.lockedUntil)
    .run();
  return decision;
}

async function redeemJoinCode(env, request, hash, deviceId, deviceName) {
  const row = await env.DB.prepare(
    `SELECT c.used_at, c.revoked_at, c.expires_at, c.id AS code_id, n.*
       FROM join_codes c JOIN networks n ON n.id = c.network_id
      WHERE c.code_hash = ?`,
  )
    .bind(hash)
    .first();
  if (!row) return null;
  const t = now();
  if (!joinRedeemable({ usedAt: row.used_at, revokedAt: row.revoked_at, expiresAt: row.expires_at, now: t })) {
    await recordGuess(env, request, "join", false);
    throw new HttpError(400, "That code is not valid.");
  }
  const claimed = await env.DB.prepare(
    `UPDATE join_codes SET used_at = ?
      WHERE code_hash = ? AND used_at IS NULL AND revoked_at IS NULL AND expires_at > ?`,
  )
    .bind(t, hash, t)
    .run();
  if (!claimed.meta || !claimed.meta.changes) {
    await recordGuess(env, request, "join", false);
    throw new HttpError(400, "That code is not valid.");
  }
  const owner = await env.DB.prepare("SELECT id, email FROM accounts WHERE id = ?").bind(row.owner_id).first();
  if (!owner) throw new HttpError(400, "That code is not valid.");
  await recordGuess(env, request, "join", true);
  return json(await issue(env, { accountId: owner.id, email: owner.email }, row, deviceId, deviceName, null));
}

async function listJoinCodes(env, account) {
  const row = await owned(env, account.accountId);
  if (!row) throw new HttpError(404, "Set your network before creating a code.");
  const t = now();
  const { results } = await env.DB.prepare(
    `SELECT id, created_at, expires_at FROM join_codes
      WHERE network_id = ? AND used_at IS NULL AND revoked_at IS NULL AND expires_at > ?
      ORDER BY created_at DESC`,
  )
    .bind(row.id, t)
    .all();
  return json({
    codes: (results || []).map((item) => ({
      id: item.id,
      created_at: item.created_at,
      expires_at: item.expires_at,
    })),
  });
}

async function createJoinCode(env, account) {
  const row = await owned(env, account.accountId);
  if (!row) throw new HttpError(404, "Set your network before creating a code.");
  const t = now();
  await env.DB.prepare(
    `UPDATE join_codes SET revoked_at = ?
      WHERE network_id = ? AND used_at IS NULL AND revoked_at IS NULL`,
  )
    .bind(t, row.id)
    .run();
  const code = randomDigits(8);
  const id = `jnc_${randomHex(8)}`;
  const expires = t + JOIN_TTL_SEC;
  await env.DB.prepare(
    "INSERT INTO join_codes (id, code_hash, network_id, created_at, expires_at) VALUES (?, ?, ?, ?, ?)",
  )
    .bind(id, await sha256Hex(code), row.id, t, expires)
    .run();
  return json({ id, code, expires_at: expires });
}

async function revokeJoinCode(request, env, account) {
  const row = await owned(env, account.accountId);
  if (!row) throw new HttpError(404, "No network.");
  const body = await readJson(request);
  const id = String(body.id || "");
  if (!id.startsWith("jnc_")) throw new HttpError(400, "Unknown code.");
  const t = now();
  await env.DB.prepare(
    `UPDATE join_codes SET revoked_at = ?
      WHERE id = ? AND network_id = ? AND used_at IS NULL AND revoked_at IS NULL`,
  )
    .bind(t, id, row.id)
    .run();
  return json({ ok: true, id });
}

function pjdeskIdOk(id) {
  return /^\d{9}$/.test(String(id || ""));
}

async function supportOffer(request, env) {
  await assertGuessOpen(env, request, "support-publish");
  const body = await readJson(request);
  const codeHash = String(body.code_hash || "").trim().toLowerCase();
  const pjdeskId = String(body.pjdesk_id || "").replace(/\D/g, "");
  if (!/^[0-9a-f]{64}$/.test(codeHash) || !pjdeskIdOk(pjdeskId)) {
    throw new HttpError(400, "That support code is not valid.");
  }
  const decision = await recordGuess(env, request, "support-publish", false, {
    limit: 240,
    windowSec: 60 * 60,
    lockSec: 60,
  });
  if (decision.locked) throw new HttpError(429, decision.message);
  const t = now();
  await env.DB.batch([
    env.DB.prepare("DELETE FROM support_offers WHERE pjdesk_id = ? OR expires_at <= ?").bind(pjdeskId, t),
    env.DB.prepare(
      "INSERT INTO support_offers (code_hash, pjdesk_id, created_at, expires_at) VALUES (?, ?, ?, ?)",
    ).bind(codeHash, pjdeskId, t, t + SUPPORT_TTL_SEC),
  ]);
  return json({ ok: true, expires_at: t + SUPPORT_TTL_SEC });
}

async function supportRevoke(request, env) {
  const body = await readJson(request);
  const codeHash = String(body.code_hash || "").trim().toLowerCase();
  const pjdeskId = String(body.pjdesk_id || "").replace(/\D/g, "");
  if (!/^[0-9a-f]{64}$/.test(codeHash) || !pjdeskIdOk(pjdeskId)) {
    throw new HttpError(400, "That support code is not valid.");
  }
  await env.DB.prepare("DELETE FROM support_offers WHERE code_hash = ? AND pjdesk_id = ?")
    .bind(codeHash, pjdeskId)
    .run();
  return json({ ok: true });
}

async function supportLookup(request, env) {
  await assertGuessOpen(env, request, "support");
  const body = await readJson(request);
  const code = String(body.code || "").replace(/\D/g, "");
  if (code.length !== 8) {
    await recordGuess(env, request, "support", false);
    throw new HttpError(400, "That code is not valid.");
  }
  const hash = await sha256Hex(code);
  const row = await env.DB.prepare("SELECT pjdesk_id, expires_at FROM support_offers WHERE code_hash = ?")
    .bind(hash)
    .first();
  const t = now();
  if (!row || row.expires_at <= t || !pjdeskIdOk(row.pjdesk_id)) {
    await recordGuess(env, request, "support", false);
    throw new HttpError(400, "That code is not valid.");
  }
  await recordGuess(env, request, "support", true);
  return json({ id: row.pjdesk_id });
}

async function policy(env, account) {
  const found = await networkFor(env, account);
  if (!found.network) throw new HttpError(404, "No network.");
  const network = found.network;
  const { publicKey } = await signingKey(env);
  const members = await env.DB.prepare(
    "SELECT email FROM network_members WHERE network_id = ? AND revoked_at IS NULL",
  )
    .bind(network.id)
    .all();
  const owner = await env.DB.prepare("SELECT email FROM accounts WHERE id = ?").bind(network.owner_id).first();
  const allowed = [owner?.email, ...(members.results || []).map((r) => r.email)].filter(Boolean);
  const revoked = await env.DB.prepare(
    "SELECT device_id FROM network_devices WHERE network_id = ? AND revoked_at IS NOT NULL",
  )
    .bind(network.id)
    .all();
  return json({
    network_id: network.id,
    require_accounts: requireAccountsOn(network.require_accounts),
    public_key: publicKey,
    allowed_accounts: [...new Set(allowed)],
    revoked_devices: (revoked.results || []).map((r) => r.device_id),
    fingerprints: fingerprintsOf(network),
  });
}

async function invitesFor(env, networkId) {
  const { results } = await env.DB.prepare(
    `SELECT id, mode, bound_email, max_uses, uses, expires_at, created_at, revoked_at
       FROM network_invites WHERE network_id = ? ORDER BY created_at DESC`,
  )
    .bind(networkId)
    .all();
  const uses = await env.DB.prepare(
    `SELECT u.invite_id, u.device_id, u.device_name, u.email, u.used_at
       FROM network_invite_uses u JOIN network_invites i ON i.id = u.invite_id
      WHERE i.network_id = ? ORDER BY u.used_at DESC`,
  )
    .bind(networkId)
    .all();
  const by = {};
  for (const row of uses.results || []) {
    (by[row.invite_id] ||= []).push({
      device_id: row.device_id,
      device_name: row.device_name,
      email: row.email,
      used_at: row.used_at,
    });
  }
  return (results || []).map((row) => ({
    id: row.id,
    mode: row.mode,
    bound_email: row.bound_email || "",
    max_uses: row.max_uses,
    uses: row.uses || 0,
    expires_at: row.expires_at,
    created_at: row.created_at,
    revoked_at: row.revoked_at,
    history: by[row.id] || [],
  }));
}

async function createInvite(request, env, account) {
  const row = await owned(env, account.accountId);
  if (!row) throw new HttpError(404, "Set your network before creating a network key.");
  if (!row.active_address) throw new HttpError(400, NO_SERVER_MESSAGE);
  const body = await readJson(request);
  const secretHash = String(body.secret_hash || "").trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(secretHash)) throw new HttpError(400, "This network key is not valid.");
  const mode = body.mode === "embedded" ? "embedded" : body.mode === "passphrase" ? "passphrase" : "";
  if (!mode) throw new HttpError(400, "Choose how this network key is protected.");
  const bound = normEmail(body.bound_email || "");
  if (bound && !validEmail(bound)) throw new HttpError(400, "Enter a valid email address.");
  if (bound && bound !== account.email) {
    const member = await env.DB.prepare(
      "SELECT revoked_at FROM network_members WHERE network_id = ? AND email = ?",
    )
      .bind(row.id, bound)
      .first();
    if (!member || member.revoked_at) throw new HttpError(400, "That account is not allowed on this network.");
  }
  let maxUses = null;
  if (body.max_uses != null && body.max_uses !== "") {
    maxUses = Number(body.max_uses);
    if (!Number.isInteger(maxUses) || maxUses < 1 || maxUses > 10000) {
      throw new HttpError(400, "Maximum uses has to be at least 1, or leave it empty.");
    }
  }
  let expiresAt = null;
  if (body.expires_at != null && body.expires_at !== "") {
    expiresAt = Number(body.expires_at);
    if (!Number.isInteger(expiresAt) || expiresAt <= now()) throw new HttpError(400, "That expiry is already past.");
  }
  const id = `ink_${randomHex(8)}`;
  await env.DB.prepare(
    `INSERT INTO network_invites (id, network_id, secret_hash, mode, bound_email, max_uses, uses, expires_at, created_at, revoked_at)
     VALUES (?, ?, ?, ?, ?, ?, 0, ?, ?, NULL)`,
  )
    .bind(id, row.id, secretHash, mode, bound, maxUses, expiresAt, now())
    .run();
  return json({ id });
}

async function revokeInvite(request, env, account) {
  const row = await owned(env, account.accountId);
  if (!row) throw new HttpError(404, "No network.");
  const { id } = await readJson(request);
  if (typeof id !== "string" || !id.startsWith("ink_")) throw new HttpError(400, "Unknown network key.");
  const t = now();
  await env.DB.prepare("UPDATE network_invites SET revoked_at = ? WHERE id = ? AND network_id = ? AND revoked_at IS NULL")
    .bind(t, id, row.id)
    .run();
  await env.DB.prepare(
    `UPDATE network_devices SET revoked_at = ?, refresh_hash = ''
      WHERE network_id = ? AND revoked_at IS NULL
        AND device_id IN (SELECT device_id FROM network_invite_uses WHERE invite_id = ?)`,
  )
    .bind(t, row.id, id)
    .run();
  return getNetwork(env, account);
}

async function redeem(request, env) {
  const body = await readJson(request);
  const secret = String(body.secret || "").trim().toLowerCase();
  const deviceId = String(body.device_id || "");
  if (!/^[0-9a-f]{64}$/.test(secret)) throw new HttpError(400, "This network key is not valid.");
  const hash = await sha256Hex(secret);
  const row = await env.DB.prepare(
    `SELECT i.*, n.owner_id, n.active_address, n.active_fingerprint, n.standby_address, n.standby_fingerprint, n.require_accounts, n.name, n.id AS nid
       FROM network_invites i JOIN networks n ON n.id = i.network_id
      WHERE i.secret_hash = ?`,
  )
    .bind(hash)
    .first();
  if (!row) throw new HttpError(400, "This network key is not valid.");
  let caller = null;
  const header = request.headers.get("authorization") || "";
  if (header) {
    try {
      caller = await session(request, env);
    } catch {
      caller = null;
    }
  }
  const reason = inviteError({
    revoked: !!row.revoked_at,
    expiresAt: row.expires_at,
    uses: row.uses || 0,
    maxUses: row.max_uses,
    now: now(),
    boundEmail: row.bound_email,
    callerEmail: caller?.email || "",
  });
  if (reason) throw new HttpError(400, reason);
  if (row.bound_email) {
    const member = await env.DB.prepare(
      "SELECT revoked_at FROM network_members WHERE network_id = ? AND email = ?",
    )
      .bind(row.network_id, row.bound_email)
      .first();
    const owner = await env.DB.prepare("SELECT email FROM accounts WHERE id = ?").bind(row.owner_id).first();
    if (owner?.email !== row.bound_email && (!member || member.revoked_at)) {
      throw new HttpError(400, "This network key was revoked.");
    }
  }
  const claimed = await env.DB.prepare(
    `UPDATE network_invites SET uses = uses + 1
      WHERE id = ? AND revoked_at IS NULL AND (max_uses IS NULL OR uses < max_uses)`,
  )
    .bind(row.id)
    .run();
  if (!claimed.meta || !claimed.meta.changes) {
    throw new HttpError(400, "This network key has already been used the maximum number of times.");
  }
  const owner = await env.DB.prepare("SELECT id, email FROM accounts WHERE id = ?").bind(row.owner_id).first();
  if (!owner) throw new HttpError(400, "This network key is not valid.");
  const enrollAccount = row.bound_email && caller ? caller : { accountId: owner.id, email: owner.email };
  const network = {
    id: row.network_id,
    active_address: row.active_address,
    active_fingerprint: row.active_fingerprint,
    standby_address: row.standby_address,
    standby_fingerprint: row.standby_fingerprint,
  };
  const issued = await issue(env, enrollAccount, network, deviceId, body.device_name || "Network key", null);
  await env.DB.prepare(
    "INSERT INTO network_invite_uses (invite_id, device_id, device_name, email, used_at) VALUES (?, ?, ?, ?, ?)",
  )
    .bind(row.id, deviceId, String(body.device_name || "").slice(0, 80), enrollAccount.email, now())
    .run();
  return json(issued);
}
