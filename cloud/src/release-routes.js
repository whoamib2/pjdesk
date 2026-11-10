import { DownloadError, requireDownload } from "./download-gate.js";
import {
  adminOk,
  downloader,
  githubUrls,
  json,
  manifestUrls,
  pickKind,
  publicFallback,
  publishVersion,
  putReleaseFile,
  readLatest,
  requireDownloader,
  safeFile,
  safeVersion,
  serveReleaseFile,
  signManifestTickets,
  siteOrigin,
  ticketSecret,
} from "./release-store.js";

function want(request, name) {
  const params = new URL(request.url).searchParams;
  const v = params.get(name) || request.headers.get(`x-content-${name}`) || "";
  return v;
}

async function bodySha256(bytes) {
  return [...new Uint8Array(await crypto.subtle.digest("SHA-256", bytes))]
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

export async function handleReleaseApi(request, env, url, accountOk) {
  const p = url.pathname;
  const m = request.method;

  if (p === "/api/release" && m === "GET") return apiRelease(request, env, url, accountOk);

  if (p === "/api/admin/releases" && m === "GET") {
    if (!(await adminOk(request, env))) throw new DownloadError(401, "bad admin key", "unauthorized");
    const latest = await readLatest(env);
    return json({ latest });
  }

  const up = /^\/api\/admin\/releases\/([^/]+)\/files\/([^/]+)$/.exec(p);
  if (up && m === "PUT") {
    if (!(await adminOk(request, env))) throw new DownloadError(401, "bad admin key", "unauthorized");
    const version = decodeURIComponent(up[1]);
    const name = decodeURIComponent(up[2]);
    if (!safeVersion(version) || !safeFile(name)) throw new DownloadError(400, "Bad file name.", "invalid_name");
    if (!request.body) throw new DownloadError(400, "empty body", "invalid_body");
    const bytes = new Uint8Array(await request.arrayBuffer());
    const got = await bodySha256(bytes);
    const wantHash = (want(request, "sha256") || "").toLowerCase();
    if (wantHash && wantHash !== got) {
      throw new DownloadError(400, `SHA-256 mismatch for ${name}`, "checksum_mismatch");
    }
    const info = await putReleaseFile(env, version, name, bytes, got, bytes.length);
    return json(info);
  }

  const pub = /^\/api\/admin\/releases\/([^/]+)\/publish$/.exec(p);
  if (pub && m === "POST") {
    if (!(await adminOk(request, env))) throw new DownloadError(401, "bad admin key", "unauthorized");
    const version = decodeURIComponent(pub[1]);
    let files = [];
    try {
      const body = await request.json();
      files = Array.isArray(body.files) ? body.files : [];
    } catch {
      throw new DownloadError(400, "Bad request.", "invalid_body");
    }
    if (!files.length) throw new DownloadError(400, "files required", "invalid_body");
    const manifest = await publishVersion(env, version, files);
    return json({ ok: true, ...manifest });
  }

  return null;
}

export async function handleDl(request, env, url, accountOk) {
  const parts = url.pathname.split("/").filter(Boolean);
  // /dl/latest/<file> or /dl/<version>/<file>
  if (parts[0] !== "dl" || parts.length < 3) {
    throw new DownloadError(404, "Not found.", "not_found");
  }
  let version = decodeURIComponent(parts[1]);
  let name = decodeURIComponent(parts.slice(2).join("/"));
  const latest = await readLatest(env);
  if (version === "latest") {
    if (!latest) throw new DownloadError(404, "No release yet.", "none");
    version = latest.version;
    const alias = pickKind(latest.files, name);
    if (alias) name = alias;
  }
  if (!safeVersion(version) || !safeFile(name)) throw new DownloadError(400, "Bad file name.", "invalid_name");
  await requireDownloader(request, env, version, name, accountOk);
  return serveReleaseFile(env, version, name, request);
}

export async function redirectDownload(request, env, url, accountOk) {
  await requireDownload(request, env);
  const what = url.pathname.split("/")[2] || "";
  const latest = await readLatest(env);
  if (latest) {
    const name = pickKind(latest.files, what === "sums" ? "sums" : what);
    if (name) {
      return Response.redirect(`${siteOrigin(url)}/dl/${encodeURIComponent(latest.version)}/${encodeURIComponent(name)}`, 302);
    }
  }
  // Nothing published to R2 yet: the caller falls back to the GitHub redirect.
  return null;
}

async function apiRelease(request, env, url, accountOk) {
  const who = await downloader(request, env, "", "", accountOk);
  const latest = await readLatest(env);
  const repo = env.RELEASE_REPO || "whoamib2/pjdesk";

  if (who && latest) {
    const tickets = ticketSecret(env) ? await signManifestTickets(env, latest) : {};
    const info = manifestUrls(siteOrigin(url), latest, tickets);
    const etag = `"${latest.version}-auth"`;
    return tagged(request, info, etag, "private, no-store");
  }

  // Unauthenticated: keep GitHub URLs so 0.6.90 and older can still update
  // while the repo is public. Do not return pjdesk.com /dl/ URLs here:
  // 0.3.7–0.6.90 refuse any non-GitHub host before they GET the file, and a
  // ?t= ticket also breaks 0.6.90 server filename parsing. After the repo
  // is private those old installs cannot be kept updating. When fallback is
  // off, still name the version so a current install sees it is up to date,
  // but leave the file URLs empty instead of pointing at a locked or dead
  // link (a failed update must not take a machine offline).
  const version = latest?.version;
  if (!publicFallback(env)) {
    const info = {
      version: version || "",
      client: "",
      server: "",
      setup: "",
      apk: "",
      sums: "",
      cloud: "",
      notes: "",
    };
    if (!version) {
      // Fall back to the previous GitHub lookup only when nothing is published
      // in R2 yet, so a mid-migration Worker does not break updates.
      return null;
    }
    return tagged(request, info, `"${version}-closed"`, "private, no-store");
  }

  if (latest) {
    const info = githubUrls(repo, latest.version);
    return tagged(request, info, `"${latest.version}-public"`, "public, max-age=300");
  }
  // Nothing in R2 yet: the existing GitHub lookup in index.js still runs.
  return null;
}

function tagged(request, info, etag, cache) {
  const headers = { etag, "cache-control": cache };
  const inm = request.headers.get("if-none-match") || "";
  if (inm.split(",").some((part) => part.trim() === etag || part.trim() === `W/${etag}`)) {
    return new Response(null, { status: 304, headers });
  }
  return json(info, 200, headers);
}

