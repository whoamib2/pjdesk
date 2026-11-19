#!/usr/bin/env node
// Upload the six PJDesk release files into R2 through the Worker, then publish
// them as the latest release. This is the path the Windows publish pipeline
// should call. It does not deploy the Worker, create the bucket, or set secrets.
//
//   set RELEASE_ADMIN_KEY=...   (same secret as the Worker)
//   or leave it unset and the script reads %USERPROFILE%\pjdesk-release-admin-key.txt
//   (or $HOME/pjdesk-release-admin-key.txt, or PJDESK_RELEASE_ADMIN_KEY_FILE).
//   The key is never printed.
//   node tools/publish-release.mjs --dir path\to\release-0.6.91 --version 0.6.91
//
// Optional: --site https://pjdesk.com

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { readdir, readFile, stat } from "node:fs/promises";
import { join } from "node:path";

function arg(name, fallback = "") {
  const i = process.argv.indexOf(`--${name}`);
  if (i >= 0 && process.argv[i + 1]) return process.argv[i + 1];
  return fallback;
}

function fail(message) {
  console.error(message);
  process.exit(1);
}

function sha256(buf) {
  return createHash("sha256").update(buf).digest("hex");
}

function pick(names, pred) {
  return names.find(pred) || "";
}

function loadAdminKey() {
  const fromEnv = (process.env.RELEASE_ADMIN_KEY || "").trim();
  if (fromEnv) return fromEnv;
  const home = process.env.USERPROFILE || process.env.HOME || "";
  const candidates = [
    process.env.PJDESK_RELEASE_ADMIN_KEY_FILE,
    home ? join(home, "pjdesk-release-admin-key.txt") : "",
  ].filter(Boolean);
  for (const path of candidates) {
    try {
      const text = readFileSync(path, "utf8").trim();
      if (text) return text;
    } catch {
      // try the next path
    }
  }
  return "";
}

const site = (arg("site", process.env.PJDESK_SITE || "https://pjdesk.com")).replace(/\/$/, "");
const version = arg("version").replace(/^v/, "");
const dir = arg("dir");
const key = loadAdminKey();

if (!version || !/^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/.test(version)) {
  fail("Pass --version 0.6.91 (the version being published).");
}
if (!dir) fail("Pass --dir pointing at the folder that holds the six release files.");
if (!key) fail("Set RELEASE_ADMIN_KEY, or create pjdesk-release-admin-key.txt in your home folder (do not commit it).");

const names = (await readdir(dir)).filter((n) => !n.startsWith("."));
const files = {
  client: pick(names, (n) => n.startsWith("PJDesk-Client-Setup") && n.endsWith(".exe")),
  setup: pick(names, (n) => n.startsWith("PJDesk-Setup") && n.endsWith(".exe") && !n.startsWith("PJDesk-Server")),
  server: pick(names, (n) => n.startsWith("PJDesk-Server-Setup") && n.endsWith(".exe")),
  apk:
    pick(names, (n) => /^PJDesk-Android-.*\.apk$/i.test(n) && !/debug/i.test(n)) ||
    pick(names, (n) => /^PJDesk-.*\.apk$/i.test(n) && !/debug/i.test(n)),
  sums: pick(names, (n) => n === "SHA256SUMS"),
  cloud: pick(names, (n) => n === `cloud-${version}.zip` || /^cloud-.*\.zip$/.test(n)),
};

const missing = Object.entries(files)
  .filter(([, n]) => !n)
  .map(([k]) => k);
if (missing.length) {
  fail(`Missing ${missing.join(", ")} in ${dir}. Need the six release files: client, setup, server, apk, SHA256SUMS, cloud zip.`);
}

const unique = [...new Set(Object.values(files))];
const uploaded = [];
for (const name of unique) {
  const path = join(dir, name);
  const buf = await readFile(path);
  const size = (await stat(path)).size;
  const hash = sha256(buf);
  const url = `${site}/api/admin/releases/${encodeURIComponent(version)}/files/${encodeURIComponent(name)}`;
  const res = await fetch(url, {
    method: "PUT",
    headers: {
      authorization: `Admin ${key}`,
      "content-type": "application/octet-stream",
      "x-content-sha256": hash,
    },
    body: buf,
  });
  const text = await res.text();
  if (!res.ok) fail(`Upload of ${name} failed (${res.status}): ${text}`);
  uploaded.push({ name, size, sha256: hash });
  console.log(`uploaded ${name} (${size} bytes, sha256 ${hash})`);
}

const pub = await fetch(`${site}/api/admin/releases/${encodeURIComponent(version)}/publish`, {
  method: "POST",
  headers: {
    authorization: `Admin ${key}`,
    "content-type": "application/json",
  },
  body: JSON.stringify({ files: uploaded }),
});
const text = await pub.text();
if (!pub.ok) fail(`Publish failed (${pub.status}): ${text}`);
console.log(`published ${version} as latest (${unique.length} files)`);
console.log(`site ${site}/api/release`);
