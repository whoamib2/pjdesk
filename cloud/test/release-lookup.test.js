import test from "node:test";
import assert from "node:assert/strict";
import {
  RELEASE_TTL_MS,
  STALE_CACHE_URL,
  STALE_TTL_MS,
  ReleaseError,
  latestRelease,
  latestTag,
  peekReleaseMemo,
  releaseInfo,
  resetReleaseMemo,
  safeRepo,
  safeTag,
} from "../src/release-lookup.js";

const REPO = "whoamib2/pjdesk";
const TAG = "v0.6.91";
const INFO = {
  version: "0.6.91",
  client: `https://github.com/${REPO}/releases/download/${TAG}/PJDesk-Client-Setup-0.6.91.exe`,
  server: `https://github.com/${REPO}/releases/download/${TAG}/PJDesk-Server-Setup-0.6.91.exe`,
  sums: `https://github.com/${REPO}/releases/download/${TAG}/SHA256SUMS`,
  notes: `https://github.com/${REPO}/releases/tag/${TAG}`,
};

function jsonResponse(body, status = 200, headers = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json", ...headers },
  });
}

function memoryCache() {
  const map = new Map();
  return {
    default: {
      async match(url) {
        const key = String(url);
        const row = map.get(key);
        return row ? new Response(row.body, { headers: row.headers }) : undefined;
      },
      async put(url, response) {
        map.set(String(url), {
          body: await response.clone().text(),
          headers: Object.fromEntries(response.headers),
        });
      },
      store: map,
    },
  };
}

function fetchMap(routes) {
  const calls = [];
  const fn = async (url, init = {}) => {
    const method = (init.method || "GET").toUpperCase();
    calls.push({ url: String(url), method });
    const key = `${method} ${url}`;
    const handler = routes[key] ?? routes[String(url)];
    if (!handler) {
      throw new Error(`unexpected fetch ${key}`);
    }
    return typeof handler === "function" ? handler() : handler;
  };
  fn.calls = calls;
  return fn;
}

test("a repo name is owner/name, and a tag is one path segment", () => {
  assert.equal(safeRepo("whoamib2/pjdesk"), true);
  assert.equal(safeRepo("https://github.com/whoamib2/pjdesk"), false);
  assert.equal(safeTag("v0.6.91"), true);
  assert.equal(safeTag("0.6.91"), true);
  assert.equal(safeTag("v0.6.91/extra"), false);
  assert.equal(safeTag("../v1"), false);
  assert.equal(safeTag(""), false);
});

test("HEAD /releases/latest Location supplies the tag", async () => {
  const fetchImpl = fetchMap({
    [`HEAD https://github.com/${REPO}/releases/latest`]: new Response(null, {
      status: 302,
      headers: { location: `https://github.com/${REPO}/releases/tag/${TAG}` },
    }),
  });
  assert.equal(await latestTag(REPO, fetchImpl), TAG);
  assert.equal(fetchImpl.calls.length, 1);
});

test("when HEAD has no tag Location, the GitHub API is the fallback", async () => {
  const fetchImpl = fetchMap({
    [`HEAD https://github.com/${REPO}/releases/latest`]: new Response(null, { status: 200 }),
    [`GET https://api.github.com/repos/${REPO}/releases/latest`]: jsonResponse({ tag_name: TAG }),
  });
  assert.equal(await latestTag(REPO, fetchImpl), TAG);
});

test("client installer prefers PJDesk-Client-Setup when that file exists", async () => {
  const fetchImpl = fetchMap({
    [`HEAD ${INFO.client}`]: new Response(null, { status: 302 }),
  });
  const info = await releaseInfo(REPO, TAG, fetchImpl);
  assert.equal(info.client, INFO.client);
  assert.equal(info.server, INFO.server);
  assert.equal(info.sums, INFO.sums);
});

test("client installer falls back to PJDesk-Setup when the new name is missing", async () => {
  const old = `https://github.com/${REPO}/releases/download/${TAG}/PJDesk-Setup-0.6.91.exe`;
  const fetchImpl = fetchMap({
    [`HEAD ${INFO.client}`]: new Response(null, { status: 404 }),
    [`HEAD ${old}`]: new Response(null, { status: 200 }),
  });
  const info = await releaseInfo(REPO, TAG, fetchImpl);
  assert.equal(info.client, old);
});

function okGithub() {
  return fetchMap({
    [`HEAD https://github.com/${REPO}/releases/latest`]: new Response(null, {
      status: 302,
      headers: { location: `https://github.com/${REPO}/releases/tag/${TAG}` },
    }),
    [`HEAD ${INFO.client}`]: new Response(null, { status: 302 }),
  });
}

test("a successful lookup is remembered for 10 minutes and written to the Cache API", async () => {
  resetReleaseMemo();
  const caches = memoryCache();
  const fetchImpl = okGithub();
  const first = await latestRelease({ RELEASE_REPO: REPO, caches }, { fetch: fetchImpl, now: 1_000 });
  assert.deepEqual(first, INFO);
  assert.equal(peekReleaseMemo().at, 1_000);
  const cached = await caches.default.match(STALE_CACHE_URL);
  assert.ok(cached);
  const stored = JSON.parse(await cached.text());
  assert.deepEqual(stored.info, INFO);

  const second = await latestRelease(
    { RELEASE_REPO: REPO, caches },
    { fetch: fetchMap({}), now: 1_000 + RELEASE_TTL_MS - 1 },
  );
  assert.deepEqual(second, INFO);
});

test("when GitHub fails, an in-memory memo is returned even after it expires", async () => {
  resetReleaseMemo();
  const caches = memoryCache();
  await latestRelease({ RELEASE_REPO: REPO, caches }, { fetch: okGithub(), now: 1_000, retries: 0 });
  const failed = fetchMap({
    [`HEAD https://github.com/${REPO}/releases/latest`]: new Response(null, { status: 502 }),
    [`GET https://api.github.com/repos/${REPO}/releases/latest`]: jsonResponse({ message: "rate limited" }, 403),
  });
  const info = await latestRelease(
    { RELEASE_REPO: REPO, caches },
    { fetch: failed, now: 1_000 + RELEASE_TTL_MS + 1, retries: 0 },
  );
  assert.deepEqual(info, INFO);
});

test("a new isolate with no memo still serves the Cache API last-good copy", async () => {
  resetReleaseMemo();
  const caches = memoryCache();
  const at = 5_000;
  await caches.default.put(
    STALE_CACHE_URL,
    new Response(JSON.stringify({ info: INFO, at }), {
      headers: { "content-type": "application/json" },
    }),
  );
  const failed = fetchMap({
    [`HEAD https://github.com/${REPO}/releases/latest`]: () => {
      throw new Error("network down");
    },
  });
  const info = await latestRelease(
    { RELEASE_REPO: REPO, caches },
    { fetch: failed, now: at + 60_000, retries: 0 },
  );
  assert.deepEqual(info, INFO);
  assert.equal(peekReleaseMemo(), null);
});

test("with no memo and no cache, a GitHub failure is the familiar 502", async () => {
  resetReleaseMemo();
  const failed = fetchMap({
    [`HEAD https://github.com/${REPO}/releases/latest`]: new Response(null, { status: 404 }),
    [`GET https://api.github.com/repos/${REPO}/releases/latest`]: jsonResponse({ message: "Not Found" }, 404),
  });
  await assert.rejects(
    () => latestRelease({ RELEASE_REPO: REPO, caches: memoryCache() }, { fetch: failed, retries: 0 }),
    (err) => {
      assert.equal(err instanceof ReleaseError, true);
      assert.equal(err.status, 502);
      assert.equal(err.message, "Couldn't reach GitHub for the latest release.");
      return true;
    },
  );
});

test("a last-good copy older than a day is not served", async () => {
  resetReleaseMemo();
  const caches = memoryCache();
  const at = 10_000;
  await caches.default.put(
    STALE_CACHE_URL,
    new Response(JSON.stringify({ info: INFO, at }), {
      headers: { "content-type": "application/json" },
    }),
  );
  const failed = fetchMap({
    [`HEAD https://github.com/${REPO}/releases/latest`]: new Response(null, { status: 503 }),
    [`GET https://api.github.com/repos/${REPO}/releases/latest`]: jsonResponse({}, 503),
  });
  await assert.rejects(
    () =>
      latestRelease(
        { RELEASE_REPO: REPO, caches },
        { fetch: failed, now: at + STALE_TTL_MS + 1, retries: 0 },
      ),
    (err) => err instanceof ReleaseError && err.status === 502,
  );
});

test("the first failure is retried once before giving up", async () => {
  resetReleaseMemo();
  let heads = 0;
  const fetchImpl = fetchMap({
    [`HEAD https://github.com/${REPO}/releases/latest`]: () => {
      heads += 1;
      if (heads === 1) return new Response(null, { status: 502 });
      return new Response(null, {
        status: 302,
        headers: { location: `https://github.com/${REPO}/releases/tag/${TAG}` },
      });
    },
    [`GET https://api.github.com/repos/${REPO}/releases/latest`]: jsonResponse({ message: "oops" }, 502),
    [`HEAD ${INFO.client}`]: new Response(null, { status: 302 }),
  });
  const info = await latestRelease(
    { RELEASE_REPO: REPO, caches: memoryCache() },
    { fetch: fetchImpl, retries: 1, retryDelayMs: 0 },
  );
  assert.deepEqual(info, INFO);
  assert.equal(heads, 2);
});
