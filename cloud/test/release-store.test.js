import test from "node:test";
import assert from "node:assert/strict";
import { DownloadError } from "../src/download-gate.js";
import {
  TICKET_TTL,
  downloader,
  githubUrls,
  makeTicket,
  manifestUrls,
  pickKind,
  publicFallback,
  publishVersion,
  putReleaseFile,
  requireDownloader,
  serveReleaseFile,
  signTicket,
  ticketOk,
  updaterOk,
  adminOk,
} from "../src/release-store.js";
import { handleDl, handleReleaseApi, redirectDownload } from "../src/release-routes.js";
import { makeCookieValue, cookieName, setCookieHeader, UNLOCK_DAYS } from "../src/download-gate.js";

const SITE = "https://pjdesk.com";
const PASSWORD = "website-password";
const UPDATE = "updater-secret";
const ADMIN = "admin-secret";

function memoryR2() {
  const map = new Map();
  const wrap = (rec, rangeHdr) => {
    let body = rec.body;
    let offset, length;
    const raw = rangeHdr && typeof rangeHdr.get === "function" ? rangeHdr.get("range") : "";
    const m = raw && /bytes=(\d+)-(\d*)/.exec(raw);
    if (m) {
      offset = Number(m[1]);
      const end = m[2] === "" ? rec.body.length - 1 : Number(m[2]);
      length = end - offset + 1;
      body = rec.body.subarray(offset, offset + length);
    }
    return {
      body,
      size: rec.body.length,
      httpEtag: rec.etag,
      customMetadata: rec.customMetadata,
      range: offset != null ? { offset, length } : undefined,
      writeHttpMetadata(h) {
        if (rec.type) h.set("content-type", rec.type);
      },
      async text() {
        return new TextDecoder().decode(rec.body);
      },
    };
  };
  return {
    async put(key, body, opts = {}) {
      const buf = body instanceof Uint8Array ? body : new Uint8Array(await new Response(body).arrayBuffer());
      map.set(key, {
        body: buf,
        customMetadata: opts.customMetadata || {},
        type: opts.httpMetadata?.contentType,
        etag: `"${buf.length}-${key}"`,
      });
    },
    async get(key, opts = {}) {
      const rec = map.get(key);
      return rec ? wrap(rec, opts.range) : null;
    },
    async head(key) {
      const rec = map.get(key);
      return rec ? { size: rec.body.length, customMetadata: rec.customMetadata } : null;
    },
  };
}

function env(overrides = {}) {
  return {
    DOWNLOAD_PASSWORD: PASSWORD,
    UPDATE_TOKEN: UPDATE,
    RELEASE_ADMIN_KEY: ADMIN,
    RELEASE_PUBLIC_FALLBACK: "1",
    RELEASE_REPO: "whoamib2/pjdesk",
    RELEASES: memoryR2(),
    ...overrides,
  };
}

function req(path, { method = "GET", headers = {}, body, url = SITE } = {}) {
  const h = new Headers(headers);
  const init = { method, headers: h };
  if (body instanceof Uint8Array) {
    init.body = body;
  } else if (body !== undefined) {
    h.set("content-type", "application/json");
    init.body = JSON.stringify(body);
  }
  return new Request(`${url}${path}`, init);
}

async function publish(e, version = "0.6.91") {
  const client = new TextEncoder().encode("client-bytes");
  const server = new TextEncoder().encode("server-bytes");
  const apk = new TextEncoder().encode("apk-bytes");
  const sums = new TextEncoder().encode(
    "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  PJDesk-Client-Setup-0.6.91.exe\n" +
      "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  PJDesk-Server-Setup-0.6.91.exe\n" +
      "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc  PJDesk-0.6.91.apk\n",
  );
  const files = [
    { name: "PJDesk-Client-Setup-0.6.91.exe", size: client.length, sha256: "aa" },
    { name: "PJDesk-Server-Setup-0.6.91.exe", size: server.length, sha256: "bb" },
    { name: "PJDesk-0.6.91.apk", size: apk.length, sha256: "cc" },
    { name: "SHA256SUMS", size: sums.length, sha256: "dd" },
    { name: "cloud-0.6.91.zip", size: 4, sha256: "ee" },
    { name: "PJDesk-Setup-0.6.91.exe", size: client.length, sha256: "aa" },
  ];
  await putReleaseFile(e, version, "PJDesk-Client-Setup-0.6.91.exe", client, "aa", client.length);
  await putReleaseFile(e, version, "PJDesk-Server-Setup-0.6.91.exe", server, "bb", server.length);
  await putReleaseFile(e, version, "PJDesk-0.6.91.apk", apk, "cc", apk.length);
  await putReleaseFile(e, version, "SHA256SUMS", sums, "dd", sums.length);
  await putReleaseFile(e, version, "cloud-0.6.91.zip", new TextEncoder().encode("zip!"), "ee", 4);
  await putReleaseFile(e, version, "PJDesk-Setup-0.6.91.exe", client, "aa", client.length);
  await publishVersion(e, version, files);
  return files;
}

test("aliases pick the six release files", () => {
  const names = [
    "PJDesk-Client-Setup-0.6.91.exe",
    "PJDesk-Setup-0.6.91.exe",
    "PJDesk-Server-Setup-0.6.91.exe",
    "PJDesk-0.6.91.apk",
    "SHA256SUMS",
    "cloud-0.6.91.zip",
  ];
  assert.equal(pickKind(names, "client"), "PJDesk-Client-Setup-0.6.91.exe");
  assert.equal(pickKind(names, "setup"), "PJDesk-Setup-0.6.91.exe");
  assert.equal(pickKind(names, "server"), "PJDesk-Server-Setup-0.6.91.exe");
  assert.equal(pickKind(names, "apk"), "PJDesk-0.6.91.apk");
  assert.equal(pickKind(names, "sums"), "SHA256SUMS");
  assert.equal(pickKind(names, "cloud"), "cloud-0.6.91.zip");
  assert.equal(publicFallback({ RELEASE_PUBLIC_FALLBACK: "1" }), true);
  assert.equal(publicFallback({ RELEASE_PUBLIC_FALLBACK: "0" }), false);
});

test("a signed ticket works until it expires, and a forged one does not", async () => {
  const e = env();
  const t = await makeTicket(e, "0.6.91", "SHA256SUMS");
  const ok = await ticketOk(req(`/dl/0.6.91/SHA256SUMS?t=${t}`), e, "0.6.91", "SHA256SUMS");
  assert.equal(ok, true);
  const forged = await ticketOk(req(`/dl/0.6.91/SHA256SUMS?t=1999999999.${"0".repeat(64)}`), e, "0.6.91", "SHA256SUMS");
  assert.equal(forged, false);
  const other = await ticketOk(req(`/dl/0.6.91/SHA256SUMS?t=${t}`), e, "0.6.91", "other.exe");
  assert.equal(other, false);
  assert.equal((await signTicket(e, "0.6.91", "SHA256SUMS", 1)).length, 64);
  assert.equal(TICKET_TTL, 60 * 60);
});

test("updater and admin headers are accepted; a wrong scheme is not", async () => {
  const e = env();
  assert.equal(await updaterOk(req("/dl/x", { headers: { authorization: `Updater ${UPDATE}` } }), e), true);
  assert.equal(await updaterOk(req("/dl/x", { headers: { authorization: `Updater nope` } }), e), false);
  assert.equal(await adminOk(req("/dl/x", { headers: { authorization: `Admin ${ADMIN}` } }), e), true);
  assert.equal(await adminOk(req("/dl/x", { headers: { authorization: `Bearer ${ADMIN}` } }), e), false);
});

test("an unauthenticated browser is sent to the form; other callers get 401", async () => {
  const e = env();
  await assert.rejects(
    requireDownloader(req("/dl/0.6.91/SHA256SUMS", { headers: { accept: "text/html" } }), e, "0.6.91", "SHA256SUMS"),
    (err) => err instanceof DownloadError && err.status === 302 && err.location === "/#download" && err.code === "download_password",
  );
  await assert.rejects(
    requireDownloader(req("/dl/0.6.91/SHA256SUMS"), e, "0.6.91", "SHA256SUMS"),
    (err) => err instanceof DownloadError && err.status === 401 && err.code === "download_password",
  );
});

test("the download cookie, updater token, admin key, account token, or ticket can open a file", async () => {
  const e = env();
  await publish(e);
  const exp = Math.floor(Date.now() / 1000) + 3600;
  const cookie = `${cookieName(SITE)}=${await makeCookieValue(PASSWORD, exp)}`;

  let res = await handleDl(req("/dl/latest/client", { headers: { cookie } }), e, new URL(SITE + "/dl/latest/client"), async () => false);
  assert.equal(res.status, 200);
  assert.equal(await res.text(), "client-bytes");
  assert.equal(res.headers.get("cache-control").startsWith("private"), true);

  res = await handleDl(
    req("/dl/0.6.91/PJDesk-Server-Setup-0.6.91.exe", { headers: { authorization: `Updater ${UPDATE}` } }),
    e,
    new URL(SITE + "/dl/0.6.91/PJDesk-Server-Setup-0.6.91.exe"),
    async () => false,
  );
  assert.equal(await res.text(), "server-bytes");

  res = await handleDl(
    req("/dl/latest/apk", { headers: { authorization: `Admin ${ADMIN}` } }),
    e,
    new URL(SITE + "/dl/latest/apk"),
    async () => false,
  );
  assert.equal(await res.text(), "apk-bytes");

  res = await handleDl(
    req("/dl/latest/sums", { headers: { authorization: "Bearer account-session" } }),
    e,
    new URL(SITE + "/dl/latest/sums"),
    async () => true,
  );
  assert.equal(res.status, 200);

  const t = await makeTicket(e, "0.6.91", "cloud-0.6.91.zip");
  res = await handleDl(req(`/dl/0.6.91/cloud-0.6.91.zip?t=${t}`), e, new URL(`${SITE}/dl/0.6.91/cloud-0.6.91.zip?t=${t}`), async () => false);
  assert.equal(await res.text(), "zip!");

  const ranged = await handleDl(
    req("/dl/latest/client", { headers: { cookie, range: "bytes=0-5" } }),
    e,
    new URL(SITE + "/dl/latest/client"),
    async () => false,
  );
  assert.equal(ranged.status, 206);
  assert.equal(await ranged.text(), "client");
});

test("authenticated /api/release returns pjdesk.com /dl/ URLs with tickets; unauthenticated keeps GitHub URLs", async () => {
  const e = env();
  await publish(e);
  const authed = await handleReleaseApi(
    req("/api/release", { headers: { authorization: `Updater ${UPDATE}` } }),
    e,
    new URL(SITE + "/api/release"),
    async () => false,
  );
  const a = await authed.json();
  assert.equal(a.version, "0.6.91");
  assert.match(a.client, /^https:\/\/pjdesk.com\/dl\/0\.6\.91\/PJDesk-Client-Setup-0\.6\.91\.exe\?t=/);
  assert.match(a.server, /\/dl\/0\.6\.91\/PJDesk-Server-Setup-0\.6\.91\.exe\?t=/);
  assert.match(a.apk, /\/dl\/0\.6\.91\/PJDesk-0\.6\.91\.apk\?t=/);
  assert.match(a.sums, /\/dl\/0\.6\.91\/SHA256SUMS\?t=/);

  const open = await handleReleaseApi(req("/api/release"), e, new URL(SITE + "/api/release"), async () => false);
  const g = await open.json();
  assert.equal(g.version, "0.6.91");
  assert.equal(g.client, "https://github.com/whoamib2/pjdesk/releases/download/v0.6.91/PJDesk-Client-Setup-0.6.91.exe");
  assert.equal(g.server, "https://github.com/whoamib2/pjdesk/releases/download/v0.6.91/PJDesk-Server-Setup-0.6.91.exe");
  assert.equal(g.sums, "https://github.com/whoamib2/pjdesk/releases/download/v0.6.91/SHA256SUMS");

  const closed = env({ RELEASE_PUBLIC_FALLBACK: "0", RELEASES: e.RELEASES });
  const after = await handleReleaseApi(req("/api/release"), closed, new URL(SITE + "/api/release"), async () => false);
  const c = await after.json();
  assert.equal(c.version, "0.6.91");
  assert.equal(c.server, "");
  assert.equal(c.client, "");
  assert.equal(c.sums, "");
});

test("the release tool can upload and publish the six files", async () => {
  const e = env();
  const body = new TextEncoder().encode("hello-sums");
  const hash = [...new Uint8Array(await crypto.subtle.digest("SHA-256", body))].map((b) => b.toString(16).padStart(2, "0")).join("");
  const put = await handleReleaseApi(
    req("/api/admin/releases/0.6.91/files/SHA256SUMS", {
      method: "PUT",
      headers: { authorization: `Admin ${ADMIN}`, "x-content-sha256": hash },
      body,
    }),
    e,
    new URL(SITE + "/api/admin/releases/0.6.91/files/SHA256SUMS"),
    async () => false,
  );
  assert.equal(put.status, 200);
  const pub = await handleReleaseApi(
    req("/api/admin/releases/0.6.91/publish", {
      method: "POST",
      headers: { authorization: `Admin ${ADMIN}` },
      body: { files: [{ name: "SHA256SUMS", size: body.length, sha256: hash }] },
    }),
    e,
    new URL(SITE + "/api/admin/releases/0.6.91/publish"),
    async () => false,
  );
  const out = await pub.json();
  assert.equal(out.ok, true);
  assert.equal(out.version, "0.6.91");

  await assert.rejects(
    handleReleaseApi(
      req("/api/admin/releases/0.6.91/publish", { method: "POST", body: { files: [{ name: "SHA256SUMS" }] } }),
      e,
      new URL(SITE + "/api/admin/releases/0.6.91/publish"),
      async () => false,
    ),
    (err) => err instanceof DownloadError && err.status === 401,
  );
});

test("a website button with the cookie is sent to /dl of the published file", async () => {
  const e = env();
  await publish(e);
  const exp = Math.floor(Date.now() / 1000) + 60;
  const cookie = `${cookieName(SITE)}=${await makeCookieValue(PASSWORD, exp)}`;
  const res = await redirectDownload(
    req("/download/client", { headers: { cookie } }),
    e,
    new URL(SITE + "/download/client"),
    async () => false,
  );
  assert.equal(res.status, 302);
  assert.equal(res.headers.get("location"), "https://pjdesk.com/dl/0.6.91/PJDesk-Client-Setup-0.6.91.exe");
});

test("githubUrls stay on the public repo during the transition", () => {
  const u = githubUrls("whoamib2/pjdesk", "0.6.90");
  assert.equal(u.server, "https://github.com/whoamib2/pjdesk/releases/download/v0.6.90/PJDesk-Server-Setup-0.6.90.exe");
  assert.equal(u.apk, "https://github.com/whoamib2/pjdesk/releases/download/v0.6.90/PJDesk-0.6.90.apk");
});

test("downloader reports each kind of access and nobody for a bare request", async () => {
  const e = env();
  assert.equal(await downloader(req("/dl/x"), e, "0.6.91", "SHA256SUMS"), null);
  assert.equal(await downloader(req("/dl/x", { headers: { authorization: `Updater ${UPDATE}` } }), e, "0.6.91", "SHA256SUMS"), "updater");
});
