// Latest PJDesk release for /api/release and /download/*.
//
// Primary lookup is HEAD https://github.com/<repo>/releases/latest (the Location
// header), not the GitHub API: anonymous API limits are shared by every Worker
// on the same Cloudflare address. If that fails, we try the API once. Installer
// URLs follow a fixed name pattern; we only HEAD the client asset to prefer the
// current PJDesk-Client-Setup name over the older PJDesk-Setup name.
//
// A 10-minute in-memory memo avoids extra GitHub calls in one isolate. Because
// that memo is lost on a new isolate (and right after a deploy), a last-good
// copy is also stored in the Cache API so a brief GitHub blip — common while a
// new release is marked Latest — does not turn into "Couldn't reach GitHub".

export const RELEASE_TTL_MS = 10 * 60 * 1000;
export const STALE_TTL_MS = 24 * 60 * 60 * 1000;
export const STALE_CACHE_URL = "https://pjdesk.com/__internal/release-memo";
export const GITHUB_UA = "pjdesk-cloud";

let releaseMemo = null; // { info, at }

export class ReleaseError extends Error {
  constructor(status, message) {
    super(message);
    this.status = status;
  }
}

export function resetReleaseMemo() {
  releaseMemo = null;
}

export function peekReleaseMemo() {
  return releaseMemo;
}

export function safeRepo(repo) {
  return typeof repo === "string" && /^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/.test(repo);
}

/** Release tags become part of a redirect URL, so they stay a single path segment. */
export function safeTag(tag) {
  return typeof tag === "string" && /^v?[0-9A-Za-z][0-9A-Za-z._+-]{0,63}$/.test(tag) && !tag.includes("..");
}

async function assetExists(url, fetchImpl) {
  try {
    const res = await fetchImpl(url, { method: "HEAD", redirect: "manual", headers: { "user-agent": GITHUB_UA } });
    return res.status === 200 || res.status === 301 || res.status === 302;
  } catch {
    return false;
  }
}

export async function releaseInfo(repo, tag, fetchImpl = fetch) {
  const version = tag.replace(/^v/, "");
  const base = `https://github.com/${repo}/releases/download/${tag}`;
  const clientNew = `${base}/PJDesk-Client-Setup-${version}.exe`;
  const clientOld = `${base}/PJDesk-Setup-${version}.exe`;
  let client = clientNew;
  if (!(await assetExists(clientNew, fetchImpl)) && (await assetExists(clientOld, fetchImpl))) client = clientOld;
  return {
    version,
    client,
    server: `${base}/PJDesk-Server-Setup-${version}.exe`,
    sums: `${base}/SHA256SUMS`,
    notes: `https://github.com/${repo}/releases/tag/${tag}`,
  };
}

export async function latestTag(repo, fetchImpl = fetch) {
  const res = await fetchImpl(`https://github.com/${repo}/releases/latest`, {
    method: "HEAD",
    redirect: "manual",
    headers: { "user-agent": GITHUB_UA },
  });
  const location = res.headers.get("location") || "";
  const m = /\/releases\/tag\/([^/?#]+)/.exec(location);
  if (m) return decodeURIComponent(m[1]);
  console.error(`release latest HEAD status=${res.status} location=${location.slice(0, 200)}`);
  const api = await fetchImpl(`https://api.github.com/repos/${repo}/releases/latest`, {
    headers: { "user-agent": GITHUB_UA, accept: "application/vnd.github+json" },
  });
  if (api.ok) {
    const r = await api.json();
    if (r.tag_name) return String(r.tag_name);
    console.error(`release latest API status=${api.status} but no tag_name`);
  } else {
    console.error(`release latest API status=${api.status}`);
  }
  return null;
}

async function loadShared(caches) {
  try {
    const cache = caches?.default;
    if (!cache) return null;
    const res = await cache.match(STALE_CACHE_URL);
    if (!res) return null;
    const data = await res.json();
    if (!data || !data.info || typeof data.at !== "number") return null;
    if (!data.info.version || !data.info.client || !data.info.server) return null;
    return data;
  } catch {
    return null;
  }
}

async function saveShared(caches, info, at) {
  try {
    const cache = caches?.default;
    if (!cache) return;
    const body = JSON.stringify({ info, at });
    await cache.put(
      STALE_CACHE_URL,
      new Response(body, {
        headers: {
          "content-type": "application/json",
          "cache-control": `max-age=${Math.floor(STALE_TTL_MS / 1000)}`,
        },
      }),
    );
  } catch (e) {
    console.error("release memo cache write failed:", e && e.message);
  }
}

function staleInfo(memo, shared, now) {
  if (memo?.info) return memo.info;
  if (shared?.info && now - shared.at <= STALE_TTL_MS) return shared.info;
  return null;
}

/**
 * @param {object} env
 * @param {string} env.RELEASE_REPO
 * @param {object} [env.caches] Cache API; defaults to globalThis.caches
 * @param {object} [opts]
 * @param {number} [opts.now]
 * @param {typeof fetch} [opts.fetch]
 * @param {number} [opts.retries] extra attempts after the first failure (default 1)
 * @param {number} [opts.retryDelayMs] (default 200)
 */
export async function latestRelease(env, opts = {}) {
  if (!safeRepo(env?.RELEASE_REPO)) {
    throw new ReleaseError(500, "Release repository is not configured.");
  }
  const now = typeof opts.now === "number" ? opts.now : Date.now();
  const fetchImpl = opts.fetch || fetch;
  const retries = Number.isInteger(opts.retries) ? opts.retries : 1;
  const retryDelayMs = typeof opts.retryDelayMs === "number" ? opts.retryDelayMs : 200;
  const caches = env.caches || globalThis.caches;

  if (releaseMemo && now - releaseMemo.at < RELEASE_TTL_MS) return releaseMemo.info;

  let tag = null;
  const attempts = Math.max(1, 1 + retries);
  for (let i = 0; i < attempts && !safeTag(tag); i++) {
    if (i > 0 && retryDelayMs > 0) {
      await new Promise((resolve) => setTimeout(resolve, retryDelayMs));
    }
    try {
      tag = await latestTag(env.RELEASE_REPO, fetchImpl);
    } catch (e) {
      console.error("release lookup failed:", e && e.message);
    }
  }

  if (!safeTag(tag)) {
    const shared = await loadShared(caches);
    const stale = staleInfo(releaseMemo, shared, now);
    if (stale) return stale;
    throw new ReleaseError(502, "Couldn't reach GitHub for the latest release.");
  }

  const info = await releaseInfo(env.RELEASE_REPO, tag, fetchImpl);
  releaseMemo = { info, at: now };
  await saveShared(caches, info, now);
  return info;
}
