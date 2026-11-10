// Release files on R2 (bucket binding RELEASES, bucket name pjdesk-releases),
// served at /dl/<version>/<file> and /dl/latest/<file>. The bucket is only
// for installers; account sync stays in pjdesk-sync.
//
// Who may download: the download-password cookie, a signed-in account
// (Authorization: Bearer), the release tool (Authorization: Admin), an
// official updater (Authorization: Updater), or a short-lived signed ticket
// in ?t= (so Android can open a URL without custom headers).
//
// Unauthenticated GET /api/release still returns GitHub URLs while
// RELEASE_PUBLIC_FALLBACK is "1", so 0.6.90 and older can update. New
// authenticated callers get pjdesk.com /dl/ URLs.

import {
  DownloadError,
  cookieUnlocked,
  now,
  passwordSet,
  passwordsMatch,
  timingSafeEqual,
} from "./download-gate.js";

const encoder = new TextEncoder();

export const TICKET_TTL = 60 * 60;
export const VERSION_RE = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
export const FILE_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,120}$/;
export const LATEST_KEY = "releases/latest.json";

export function r2Key(version, name) {
  return `releases/${version}/${name}`;
}

export function safeVersion(v) {
  return typeof v === "string" && VERSION_RE.test(v);
}

export function safeFile(name) {
  return typeof name === "string" && FILE_RE.test(name) && !name.includes("..");
}

function toHex(buf) {
  return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

async function sha256Hex(text) {
  return toHex(await crypto.subtle.digest("SHA-256", encoder.encode(text)));
}

async function hmacHex(secret, data) {
  const key = await crypto.subtle.importKey(
    "raw",
    encoder.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  return toHex(await crypto.subtle.sign("HMAC", key, encoder.encode(data)));
}

/** Key used to sign download tickets. Changing it ends outstanding tickets. */
export function ticketSecret(env) {
  return env?.UPDATE_TOKEN || env?.DOWNLOAD_PASSWORD || env?.RELEASE_ADMIN_KEY || "";
}

export async function signTicket(env, version, name, exp) {
  const secret = ticketSecret(env);
  if (!secret) throw new DownloadError(503, "Downloads aren't open yet", "downloads_closed");
  return hmacHex(secret, `dl:${version}:${name}:${exp}`);
}

export async function makeTicket(env, version, name, exp = now() + TICKET_TTL) {
  return `${exp}.${await signTicket(env, version, name, exp)}`;
}

export async function ticketOk(request, env, version, name, at = now()) {
  if (!ticketSecret(env)) return false;
  const raw = new URL(request.url).searchParams.get("t") || "";
  const m = /^(\d{10})\.([0-9a-f]{64})$/.exec(raw);
  if (!m || Number(m[1]) < at) return false;
  return timingSafeEqual(m[2], await signTicket(env, version, name, Number(m[1])));
}

function authScheme(request) {
  const h = request.headers.get("authorization") || "";
  const m = /^(Bearer|Admin|Updater)\s+(\S+)$/i.exec(h);
  if (!m) return { scheme: "", token: "" };
  return { scheme: m[1].toLowerCase(), token: m[2] };
}

export async function updaterOk(request, env) {
  if (!env?.UPDATE_TOKEN) return false;
  const { scheme, token } = authScheme(request);
  if (scheme !== "updater" || !token) return false;
  return passwordsMatch(token, env.UPDATE_TOKEN);
}

export async function adminOk(request, env) {
  if (!env?.RELEASE_ADMIN_KEY) return false;
  const { scheme, token } = authScheme(request);
  if (scheme !== "admin" || !token) return false;
  return passwordsMatch(token, env.RELEASE_ADMIN_KEY);
}

export function publicFallback(env) {
  return String(env?.RELEASE_PUBLIC_FALLBACK || "1") !== "0";
}

/**
 * Who is asking: "password" | "account" | "admin" | "updater" | "ticket" | null.
 * `accountOk` is supplied by the Worker (validates a Bearer session).
 */
export async function downloader(request, env, version, name, accountOk) {
  if (passwordSet(env) && (await cookieUnlocked(request, env.DOWNLOAD_PASSWORD))) return "password";
  if (await updaterOk(request, env)) return "updater";
  if (await adminOk(request, env)) return "admin";
  if (accountOk && (await accountOk())) return "account";
  if (version && name && (await ticketOk(request, env, version, name))) return "ticket";
  return null;
}

export async function requireDownloader(request, env, version, name, accountOk) {
  const who = await downloader(request, env, version, name, accountOk);
  if (who) return who;
  const html =
    (request.headers.get("accept") || "").includes("text/html") ||
    request.headers.get("sec-fetch-mode") === "navigate";
  const code = passwordSet(env) || ticketSecret(env) ? "download_password" : "downloads_closed";
  const message = code === "downloads_closed" ? "Downloads aren't open yet" : "Enter the download password";
  if (html) {
    const err = new DownloadError(302, message, code);
    err.location = "/#download";
    throw err;
  }
  throw new DownloadError(401, message, code);
}

export function githubUrls(repo, version) {
  const tag = `v${String(version).replace(/^v/, "")}`;
  const ver = String(version).replace(/^v/, "");
  const base = `https://github.com/${repo}/releases/download/${tag}`;
  return {
    version: ver,
    client: `${base}/PJDesk-Client-Setup-${ver}.exe`,
    setup: `${base}/PJDesk-Setup-${ver}.exe`,
    server: `${base}/PJDesk-Server-Setup-${ver}.exe`,
    apk: `${base}/PJDesk-${ver}.apk`,
    sums: `${base}/SHA256SUMS`,
    cloud: `${base}/cloud-${ver}.zip`,
    notes: `https://github.com/${repo}/releases/tag/${tag}`,
  };
}

export function siteOrigin(url) {
  const u = new URL(url);
  if (u.hostname === "www.pjdesk.com") return "https://pjdesk.com";
  return u.origin;
}

export function siteFileUrl(origin, version, name, ticket) {
  const path = `${origin}/dl/${encodeURIComponent(version)}/${encodeURIComponent(name)}`;
  return ticket ? `${path}?t=${ticket}` : path;
}

export function pickKind(files, kind) {
  const names = files.map((f) => (typeof f === "string" ? f : f.name));
  const find = (pred) => names.find(pred);
  switch (kind) {
    case "client":
      return find((n) => n.startsWith("PJDesk-Client-Setup") && n.endsWith(".exe")) ||
        find((n) => n.startsWith("PJDesk-Setup") && n.endsWith(".exe") && !n.startsWith("PJDesk-Server"));
    case "setup":
      return find((n) => n.startsWith("PJDesk-Setup") && n.endsWith(".exe"));
    case "server":
      return find((n) => n.startsWith("PJDesk-Server-Setup") && n.endsWith(".exe"));
    case "apk":
      return (
        find((n) => /^PJDesk-Android-.*\.apk$/i.test(n) && !/debug/i.test(n)) ||
        find((n) => /^PJDesk-.*\.apk$/i.test(n) && !/debug/i.test(n))
      );
    case "sums":
      return find((n) => n === "SHA256SUMS");
    case "cloud":
      return find((n) => /^cloud-.*\.zip$/.test(n));
    default:
      return names.includes(kind) ? kind : null;
  }
}

export async function readLatest(env) {
  if (!env?.RELEASES) return null;
  const obj = await env.RELEASES.get(LATEST_KEY);
  if (!obj) return null;
  try {
    const data = JSON.parse(await obj.text());
    if (!safeVersion(data.version) || !Array.isArray(data.files)) return null;
    return data;
  } catch {
    return null;
  }
}

export async function writeLatest(env, manifest) {
  await env.RELEASES.put(LATEST_KEY, JSON.stringify(manifest), {
    httpMetadata: { contentType: "application/json" },
  });
}

export function manifestUrls(origin, manifest, tickets = {}) {
  const version = manifest.version;
  const files = manifest.files || [];
  const url = (kind) => {
    const name = pickKind(files, kind);
    if (!name) return "";
    return siteFileUrl(origin, version, name, tickets[name]);
  };
  return {
    version,
    client: url("client"),
    setup: url("setup"),
    server: url("server"),
    apk: url("apk"),
    sums: url("sums"),
    cloud: url("cloud"),
    notes: "",
  };
}

export async function signManifestTickets(env, manifest) {
  const tickets = {};
  const exp = now() + TICKET_TTL;
  for (const f of manifest.files || []) {
    const name = f.name || f;
    tickets[name] = await makeTicket(env, manifest.version, name, exp);
  }
  return tickets;
}

export async function putReleaseFile(env, version, name, body, sha256, size) {
  if (!safeVersion(version) || !safeFile(name)) throw new DownloadError(400, "Bad file name.", "invalid_name");
  const key = r2Key(version, name);
  const type = name.endsWith(".exe")
    ? "application/vnd.microsoft.portable-executable"
    : name.endsWith(".apk")
      ? "application/vnd.android.package-archive"
      : name === "SHA256SUMS"
        ? "text/plain; charset=utf-8"
        : "application/octet-stream";
  await env.RELEASES.put(key, body, {
    httpMetadata: { contentType: type, contentDisposition: `attachment; filename="${name}"` },
    customMetadata: { sha256: String(sha256 || "").toLowerCase(), size: String(size || 0) },
  });
  return { ok: true, key, size, sha256 };
}

export async function publishVersion(env, version, files) {
  if (!safeVersion(version)) throw new DownloadError(400, "Bad version.", "invalid_version");
  const manifest = { version, files, published_at: now() };
  await writeLatest(env, manifest);
  return manifest;
}

function json(body, status = 200, headers = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      "x-content-type-options": "nosniff",
      ...headers,
    },
  });
}

export async function serveReleaseFile(env, version, name, request) {
  if (!env?.RELEASES) throw new DownloadError(503, "Release storage is not configured.", "misconfigured");
  const key = r2Key(version, name);
  const head = await env.RELEASES.head(key);
  const meta = head || (await env.RELEASES.get(key));
  if (!meta) return new Response("Not found", { status: 404 });
  const size = meta.size;
  const sha = (meta.customMetadata && meta.customMetadata.sha256) || "";
  const headers = new Headers({
    "accept-ranges": "bytes",
    "cache-control": "private, max-age=86400, immutable",
    "content-disposition": `attachment; filename="${name}"`,
    "x-content-type-options": "nosniff",
  });
  if (sha) headers.set("x-content-sha256", sha);
  const range = request.headers.get("range");
  let obj;
  try {
    obj = await env.RELEASES.get(key, { range: range ? request.headers : undefined });
  } catch {
    headers.set("content-range", `bytes */${size}`);
    return new Response("Range not satisfiable", { status: 416, headers });
  }
  if (!obj) return new Response("Not found", { status: 404 });
  if (typeof obj.writeHttpMetadata === "function") obj.writeHttpMetadata(headers);
  if (obj.httpEtag) headers.set("etag", obj.httpEtag);
  headers.set("content-disposition", `attachment; filename="${name}"`);
  const r = obj.range;
  const partial = !!range && r && (r.offset !== undefined || r.length !== undefined);
  if (partial) {
    const offset = r.offset ?? 0;
    const length = r.length ?? obj.size - offset;
    headers.set("content-range", `bytes ${offset}-${offset + length - 1}/${size || obj.size}`);
    headers.set("content-length", String(length));
    return new Response(request.method === "HEAD" ? null : obj.body, { status: 206, headers });
  }
  headers.set("content-length", String(obj.size ?? size));
  return new Response(request.method === "HEAD" ? null : obj.body, { status: 200, headers });
}

export { json };
