// Account copy for a PJDesk server. Not deployed.
//
// pjdesk.com stays the source of truth for account creation, password
// changes, plans, and admin. GET /api/account/mirror returns the copy the
// primary stores. PUT applies a batch of local edits when base_revision
// matches. A mismatch is 412 with the current copy. The service-wide
// Ed25519 private key is not included. sign_pkcs8 is a per-network key.

const encoder = new TextEncoder();

function json(body, status = 200, extra = {}) {
  return new Response(JSON.stringify(body), {
    status,
    headers: {
      "content-type": "application/json; charset=utf-8",
      "cache-control": "no-store",
      ...extra,
    },
  });
}

function toHex(buf) {
  return [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

function num(value) {
  const n = Number(value);
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : 0;
}

function text(value, max) {
  return String(value || "").slice(0, max);
}

/** Stable JSON so the revision does not change when key order changes. */
export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(",")}]`;
  if (value && typeof value === "object") {
    const keys = Object.keys(value).filter((key) => value[key] !== undefined).sort();
    return `{${keys.map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(",")}}`;
  }
  return JSON.stringify(value);
}

export async function revisionOf(mirror) {
  const rest = { ...mirror };
  delete rest.revision;
  delete rest.login_fails;
  delete rest.server_keys;
  const digest = await crypto.subtle.digest("SHA-256", encoder.encode(canonical(rest)));
  return toHex(digest);
}

/**
 * Apply one local edit. Password hashes, the plan, and member revocation
 * are not in a change, so a website edit of those is left as it is.
 * Returns the copy.
 */
export function applyChange(copy, change) {
  if (!change || typeof change !== "object") return copy;
  const op = change.op;
  if (op === "computer" && change.computer) {
    const computer = change.computer;
    const id = text(computer.id, 32).replace(/\D/g, "");
    if (id.length !== 9) return copy;
    const gone = (copy.removed || []).find((row) => row.id === id);
    if (gone && num(gone.at) >= num(computer.updated_at)) return copy;
    const row = {
      id,
      account_id: text(computer.account_id, 64),
      name: text(computer.name, 80),
      hostname: text(computer.hostname, 80),
      os_family: text(computer.os_family, 32),
      os_version: text(computer.os_version, 40),
      display_name: text(computer.display_name, 80),
      starred: !!computer.starred,
      position: num(computer.position),
      hidden: !!computer.hidden,
      group: text(computer.group, 32),
      updated_at: num(computer.updated_at),
    };
    const list = copy.computers || (copy.computers = []);
    const index = list.findIndex((item) => item.id === id && item.account_id === row.account_id);
    if (index < 0) list.push(row);
    else if (row.updated_at >= num(list[index].updated_at)) list[index] = row;
  } else if (op === "remove") {
    const id = text(change.id, 32);
    const at = num(change.at);
    if (!id) return copy;
    copy.computers = (copy.computers || []).filter((row) => row.id !== id || num(row.updated_at) > at);
    const removed = copy.removed || (copy.removed = []);
    const index = removed.findIndex((row) => row.id === id);
    if (index < 0) removed.push({ id, at });
    else if (at >= num(removed[index].at)) removed[index].at = at;
  } else if (op === "vault" || op === "lock") {
    const row = change.row || {};
    const accountId = text(row.account_id, 64);
    if (!accountId) return copy;
    const key = op === "vault" ? "vaults" : "locks";
    const list = copy[key] || (copy[key] = []);
    const next = { account_id: accountId, ciphertext: text(row.ciphertext, 200_000), updated_at: num(row.updated_at) };
    const index = list.findIndex((item) => item.account_id === accountId);
    if (index < 0) list.push(next);
    else if (next.updated_at >= num(list[index].updated_at)) list[index] = next;
  } else if (op === "device" && change.device) {
    const device = change.device;
    const deviceId = text(device.device_id, 80);
    if (!deviceId || !text(device.refresh_hash, 128)) return copy;
    const list = copy.devices || (copy.devices = []);
    const next = {
      device_id: deviceId,
      account_id: text(device.account_id, 64),
      email: text(device.email, 254).toLowerCase(),
      name: text(device.name, 80),
      refresh_hash: text(device.refresh_hash, 128),
      enrolled_at: num(device.enrolled_at),
      revoked_at: num(device.revoked_at),
    };
    const index = list.findIndex((item) => item.device_id === deviceId);
    if (index < 0) list.push(next);
    else {
      const row = list[index];
      if (num(row.revoked_at) > 0 && next.revoked_at === 0 && num(row.revoked_at) >= next.enrolled_at) return copy;
      if (next.enrolled_at >= num(row.enrolled_at)) list[index] = next;
    }
  } else if (op === "revoke_device") {
    const deviceId = text(change.device_id, 80);
    const at = num(change.at);
    const row = (copy.devices || []).find((item) => item.device_id === deviceId);
    if (row && at >= num(row.enrolled_at)) {
      row.revoked_at = at;
      row.refresh_hash = "";
    }
  } else if (op === "address") {
    const address = text(change.active_address, 200);
    if (copy.network && !text(copy.network.active_address, 200) && address) {
      copy.network.active_address = address;
      copy.network.active_fingerprint = text(change.active_fingerprint, 80);
    }
  } else if (op === "sign_key") {
    const pub = text(change.public_key, 128);
    const pkcs8 = text(change.pkcs8, 400);
    if (pub && pkcs8) {
      copy.sign_public = pub;
      copy.sign_pkcs8 = pkcs8;
    }
  }
  return copy;
}

/** Empty base_revision, or any other revision, does not overwrite the website copy. */
export function decidePut(currentRevision, baseRevision) {
  if (!baseRevision || baseRevision !== currentRevision) return "conflict";
  return "apply";
}

/**
 * Build the snapshot from already-loaded rows. `cloudPublic` is the
 * service public key. The service private key is not an argument.
 */
export function assembleMirror(parts) {
  const owner = parts.owner || {};
  const network = parts.network || {};
  const credentials = [];
  if (owner.auth_hash) {
    credentials.push({
      account_id: owner.id || "",
      email: owner.email || "",
      auth_hash: owner.auth_hash,
      auth_salt: owner.auth_salt || "",
    });
  }
  for (const row of parts.memberAccounts || []) {
    if (!row.auth_hash || credentials.some((item) => item.email === row.email)) continue;
    credentials.push({
      account_id: row.id || "",
      email: row.email || "",
      auth_hash: row.auth_hash,
      auth_salt: row.auth_salt || "",
    });
  }
  return {
    revision: "",
    account_id: owner.id || "",
    email: owner.email || "",
    auth_hash: owner.auth_hash || "",
    auth_salt: owner.auth_salt || "",
    plan: owner.plan || "free",
    plan_expires_at: num(owner.plan_expires_at),
    sync_enabled: owner.sync_enabled === true || owner.sync_enabled === 1,
    network: {
      id: network.id || "",
      name: network.name || "",
      active_address: network.active_address || "",
      active_fingerprint: network.active_fingerprint || "",
      standby_address: network.standby_address || "",
      standby_fingerprint: network.standby_fingerprint || "",
      require_accounts: network.require_accounts === 1 || network.require_accounts === true,
      active_lan: network.active_lan || "",
      standby_lan: network.standby_lan || "",
      active_public_ip: network.active_public_ip || "",
      standby_public_ip: network.standby_public_ip || "",
    },
    credentials,
    devices: (parts.devices || []).map((row) => ({
      device_id: row.device_id || "",
      account_id: row.account_id || "",
      email: row.email || "",
      name: row.name || "",
      refresh_hash: row.refresh_hash || "",
      enrolled_at: num(row.enrolled_at),
      revoked_at: num(row.revoked_at),
    })),
    computers: (parts.computers || []).map((row) => ({
      id: row.device_id || row.id || "",
      account_id: row.account_id || "",
      name: row.name || "",
      hostname: row.hostname || "",
      os_family: row.os_family || "",
      os_version: row.os_version || "",
      display_name: row.display_name || "",
      starred: row.starred === 1 || row.starred === true,
      position: num(row.position),
      hidden: row.hidden === 1 || row.hidden === true,
      group: row.grp || row.group || "",
      updated_at: num(row.updated_at),
    })),
    removed: (parts.removed || []).map((row) => ({ id: row.pjdesk_id || row.id || "", at: num(row.removed_at || row.at) })),
    vaults: (parts.vaults || []).map((row) => ({
      account_id: row.account_id || "",
      ciphertext: row.ciphertext || "",
      updated_at: num(row.updated_at),
    })),
    locks: (parts.locks || []).map((row) => ({
      account_id: row.account_id || "",
      ciphertext: row.ciphertext || "",
      updated_at: num(row.updated_at),
    })),
    members: (parts.members || []).map((row) => ({
      email: row.email || "",
      account_id: row.account_id || "",
      revoked_at: num(row.revoked_at),
    })),
    invites: (parts.invites || []).map((row) => ({
      id: row.id || "",
      label: row.mode || "",
      secret_hash: row.secret_hash || "",
      expires_at: num(row.expires_at),
      revoked_at: num(row.revoked_at),
      computer_limit: num(row.max_uses),
    })),
    cloud_public: parts.cloudPublic || "",
    sign_public: parts.signPublic || "",
    sign_pkcs8: parts.signPkcs8 || "",
  };
}

async function rows(env, sql, ...binds) {
  const result = await env.DB.prepare(sql).bind(...binds).all();
  return result.results || [];
}

async function ensureSignKey(env, networkId, mirror) {
  if (mirror.sign_pkcs8) return mirror;
  const existing = await env.DB.prepare(
    "SELECT public_key, private_pkcs8 FROM network_sign_keys WHERE network_id = ?",
  )
    .bind(networkId)
    .first();
  if (existing) {
    mirror.sign_public = existing.public_key;
    mirror.sign_pkcs8 = existing.private_pkcs8;
    return mirror;
  }
  const pair = await crypto.subtle.generateKey({ name: "Ed25519" }, true, ["sign", "verify"]);
  const pub = toHex(await crypto.subtle.exportKey("raw", pair.publicKey));
  const pkcs8 = toHex(await crypto.subtle.exportKey("pkcs8", pair.privateKey));
  const now = Math.floor(Date.now() / 1000);
  await env.DB.prepare(
    "INSERT INTO network_sign_keys (network_id, public_key, private_pkcs8, created_at) VALUES (?, ?, ?, ?)",
  )
    .bind(networkId, pub, pkcs8, now)
    .run();
  mirror.sign_public = pub;
  mirror.sign_pkcs8 = pkcs8;
  return mirror;
}

async function loadParts(env, session) {
  let network = await env.DB.prepare("SELECT * FROM networks WHERE owner_id = ?").bind(session.accountId).first();
  if (!network) {
    network = await env.DB.prepare(
      `SELECT n.* FROM network_members m JOIN networks n ON n.id = m.network_id
        WHERE m.account_id = ? AND m.revoked_at IS NULL`,
    )
      .bind(session.accountId)
      .first();
  }
  if (!network) return null;
  const owner = await env.DB.prepare(
    "SELECT id, email, auth_hash, auth_salt, plan, plan_expires_at, sync_enabled FROM accounts WHERE id = ?",
  )
    .bind(network.owner_id)
    .first();
  if (!owner) return null;
  const id = network.id;
  const [members, memberAccounts, devices, computers, removed, vaults, locks, invites, cloudKey] = await Promise.all([
    rows(env, "SELECT email, account_id, revoked_at FROM network_members WHERE network_id = ?", id),
    rows(
      env,
      `SELECT a.id, a.email, a.auth_hash, a.auth_salt
         FROM network_members m JOIN accounts a ON a.email = m.email
        WHERE m.network_id = ? AND m.revoked_at IS NULL`,
      id,
    ),
    rows(
      env,
      "SELECT device_id, account_id, email, name, refresh_hash, enrolled_at, revoked_at FROM network_devices WHERE network_id = ?",
      id,
    ),
    rows(
      env,
      `SELECT account_id, device_id, name, hostname, os_family, os_version, display_name, starred, position, hidden, grp, updated_at
         FROM network_computers WHERE network_id = ?`,
      id,
    ),
    rows(env, "SELECT pjdesk_id, removed_at FROM network_removed WHERE network_id = ?", id),
    rows(env, "SELECT account_id, ciphertext, updated_at FROM network_password_vault WHERE network_id = ?", id),
    rows(env, "SELECT account_id, ciphertext, updated_at FROM network_app_lock WHERE network_id = ?", id),
    rows(env, "SELECT id, mode, secret_hash, expires_at, revoked_at, max_uses FROM network_invites WHERE network_id = ?", id),
    env.DB.prepare("SELECT public_key FROM network_keys WHERE id = 1").first(),
  ]);
  const sign = await env.DB.prepare("SELECT public_key, private_pkcs8 FROM network_sign_keys WHERE network_id = ?")
    .bind(id)
    .first();
  return { owner, network, members, memberAccounts, devices, computers, removed, vaults, locks, invites, cloudKey, sign };
}

async function snapshot(env, session) {
  const parts = await loadParts(env, session);
  if (!parts) return null;
  const mirror = assembleMirror({
    owner: parts.owner,
    network: parts.network,
    members: parts.members,
    memberAccounts: parts.memberAccounts,
    devices: parts.devices,
    computers: parts.computers,
    removed: parts.removed,
    vaults: parts.vaults,
    locks: parts.locks,
    invites: parts.invites,
    cloudPublic: parts.cloudKey?.public_key || "",
    signPublic: parts.sign?.public_key || "",
    signPkcs8: parts.sign?.private_pkcs8 || "",
  });
  await ensureSignKey(env, parts.network.id, mirror);
  mirror.revision = await revisionOf(mirror);
  return { mirror, networkId: parts.network.id };
}

function boolInt(value) {
  return value ? 1 : 0;
}

async function persist(env, networkId, change) {
  const op = change.op;
  if (op === "computer" && change.computer) {
    const c = change.computer;
    const id = text(c.id, 32).replace(/\D/g, "");
    if (id.length !== 9) return;
    await env.DB.prepare(
      `INSERT INTO network_computers
        (network_id, account_id, device_id, name, hostname, os_family, os_version, display_name, starred, position, hidden, grp, updated_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
       ON CONFLICT(network_id, account_id, device_id) DO UPDATE SET
         name = excluded.name, hostname = excluded.hostname, os_family = excluded.os_family,
         os_version = excluded.os_version, display_name = excluded.display_name, starred = excluded.starred,
         position = excluded.position, hidden = excluded.hidden, grp = excluded.grp, updated_at = excluded.updated_at
       WHERE excluded.updated_at >= network_computers.updated_at`,
    )
      .bind(
        networkId,
        text(c.account_id, 64),
        id,
        text(c.name, 80),
        text(c.hostname, 80),
        text(c.os_family, 32),
        text(c.os_version, 40),
        text(c.display_name, 80),
        boolInt(c.starred),
        num(c.position),
        boolInt(c.hidden),
        text(c.group, 32),
        num(c.updated_at),
      )
      .run();
  } else if (op === "remove") {
    const id = text(change.id, 32);
    const at = num(change.at);
    if (!id) return;
    await env.DB.prepare(
      `INSERT INTO network_removed (network_id, pjdesk_id, removed_at) VALUES (?, ?, ?)
       ON CONFLICT(network_id, pjdesk_id) DO UPDATE SET removed_at = MAX(network_removed.removed_at, excluded.removed_at)`,
    )
      .bind(networkId, id, at)
      .run();
    await env.DB.prepare(
      "DELETE FROM network_computers WHERE network_id = ? AND device_id = ? AND updated_at <= ?",
    )
      .bind(networkId, id, at)
      .run();
  } else if (op === "vault" || op === "lock") {
    const row = change.row || {};
    const table = op === "vault" ? "network_password_vault" : "network_app_lock";
    await env.DB.prepare(
      `INSERT INTO ${table} (network_id, account_id, ciphertext, updated_at) VALUES (?, ?, ?, ?)
       ON CONFLICT(network_id, account_id) DO UPDATE SET ciphertext = excluded.ciphertext, updated_at = excluded.updated_at
       WHERE excluded.updated_at >= ${table}.updated_at`,
    )
      .bind(networkId, text(row.account_id, 64), text(row.ciphertext, 200_000), num(row.updated_at))
      .run();
  } else if (op === "device" && change.device) {
    const d = change.device;
    await env.DB.prepare(
      `INSERT INTO network_devices
        (network_id, device_id, account_id, email, name, refresh_hash, enrolled_at, revoked_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, NULL)
       ON CONFLICT(network_id, device_id) DO UPDATE SET
         account_id = excluded.account_id, email = excluded.email, name = excluded.name,
         refresh_hash = excluded.refresh_hash, enrolled_at = excluded.enrolled_at, revoked_at = NULL
       WHERE network_devices.revoked_at IS NULL OR network_devices.revoked_at < excluded.enrolled_at`,
    )
      .bind(
        networkId,
        text(d.device_id, 80),
        text(d.account_id, 64),
        text(d.email, 254).toLowerCase(),
        text(d.name, 80),
        text(d.refresh_hash, 128),
        num(d.enrolled_at),
      )
      .run();
  } else if (op === "revoke_device") {
    await env.DB.prepare(
      `UPDATE network_devices SET revoked_at = ?, refresh_hash = ''
        WHERE network_id = ? AND device_id = ? AND enrolled_at <= ?`,
    )
      .bind(num(change.at), networkId, text(change.device_id, 80), num(change.at))
      .run();
  } else if (op === "address") {
    await env.DB.prepare(
      `UPDATE networks SET active_address = ?, active_fingerprint = ?
        WHERE id = ? AND active_address = ''`,
    )
      .bind(text(change.active_address, 200), text(change.active_fingerprint, 80), networkId)
      .run();
  } else if (op === "sign_key") {
    const pub = text(change.public_key, 128);
    const pkcs8 = text(change.pkcs8, 400);
    if (!pub || !pkcs8) return;
    const now = Math.floor(Date.now() / 1000);
    await env.DB.prepare(
      `INSERT INTO network_sign_keys (network_id, public_key, private_pkcs8, created_at) VALUES (?, ?, ?, ?)
       ON CONFLICT(network_id) DO UPDATE SET
         public_key = excluded.public_key,
         private_pkcs8 = excluded.private_pkcs8,
         created_at = excluded.created_at`,
    )
      .bind(networkId, pub, pkcs8, now)
      .run();
  }
}

/** GET or PUT /api/account/mirror. `session` is the signed-in account. */
export async function accountMirror(request, env, session) {
  if (request.method === "GET") {
    const loaded = await snapshot(env, session);
    if (!loaded) return json({ error: "This account has no network yet." }, 404);
    return json(loaded.mirror);
  }
  if (request.method !== "PUT") return json({ error: "Not found." }, 404);
  const raw = await request.text();
  if (raw.length > 300_000) return json({ error: "That account update is too large." }, 413);
  let body;
  try {
    body = JSON.parse(raw);
  } catch {
    return json({ error: "Bad request." }, 400);
  }
  const changes = Array.isArray(body.changes) ? body.changes.slice(0, 400) : [];
  const loaded = await snapshot(env, session);
  if (!loaded) return json({ error: "This account has no network yet." }, 404);
  if (decidePut(loaded.mirror.revision, text(body.base_revision, 80)) === "conflict") {
    return json({ mirror: loaded.mirror }, 412);
  }
  for (const change of changes) applyChange(loaded.mirror, change);
  for (const change of changes) await persist(env, loaded.networkId, change);
  const again = await snapshot(env, session);
  return json(again ? again.mirror : loaded.mirror);
}
