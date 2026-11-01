// Encrypts private signing keys before they are written to D1.
//
// Rows in network_keys (the service-wide Ed25519 key) and network_sign_keys
// (one key per network) hold `private_pkcs8`. With the SIGNING_KEY_SECRET
// Worker secret set (64 hex characters, 32 bytes), the column holds
// "enc1:" + base64(iv || AES-256-GCM ciphertext). The row's table and id are
// bound in as additional data, so a sealed value cannot be moved to another row.
//
// Transition: a row without the "enc1:" prefix is plain hex from an older
// Worker and is still read as is. sealPlainRows() rewrites such rows; the
// Worker runs it once per isolate after the secret is set. Without the secret
// nothing is sealed, and a sealed row fails with a clear error.

const PREFIX = "enc1:";
const encoder = new TextEncoder();
const decoder = new TextDecoder();
const keyCache = new Map();

export function isSealed(stored) {
  return typeof stored === "string" && stored.startsWith(PREFIX);
}

export function hasSealKey(env) {
  return /^[0-9a-fA-F]{64}$/.test(String(env?.SIGNING_KEY_SECRET || "").trim());
}

async function aesKey(env) {
  const hex = String(env.SIGNING_KEY_SECRET).trim().toLowerCase();
  let key = keyCache.get(hex);
  if (!key) {
    const raw = new Uint8Array(hex.match(/../g).map((b) => parseInt(b, 16)));
    key = await crypto.subtle.importKey("raw", raw, { name: "AES-GCM" }, false, ["encrypt", "decrypt"]);
    keyCache.clear();
    keyCache.set(hex, key);
  }
  return key;
}

function b64(bytes) {
  let s = "";
  for (const b of bytes) s += String.fromCharCode(b);
  return btoa(s);
}

function unb64(text) {
  const s = atob(text);
  const out = new Uint8Array(s.length);
  for (let i = 0; i < s.length; i++) out[i] = s.charCodeAt(i);
  return out;
}

/** Seals a hex private key for one row. Returns it unchanged when no secret is set. */
export async function sealKey(env, plain, aad) {
  if (!plain || isSealed(plain) || !hasSealKey(env)) return plain;
  const iv = crypto.getRandomValues(new Uint8Array(12));
  const ct = new Uint8Array(
    await crypto.subtle.encrypt({ name: "AES-GCM", iv, additionalData: encoder.encode(aad) }, await aesKey(env), encoder.encode(plain)),
  );
  const out = new Uint8Array(iv.length + ct.length);
  out.set(iv);
  out.set(ct, iv.length);
  return PREFIX + b64(out);
}

/** Opens a stored private key. Plain hex from an older Worker comes back as is. */
export async function openKey(env, stored, aad) {
  if (!isSealed(stored)) return stored || "";
  if (!hasSealKey(env)) throw new Error("signing key is encrypted but SIGNING_KEY_SECRET is not set");
  const bytes = unb64(stored.slice(PREFIX.length));
  const pt = await crypto.subtle.decrypt(
    { name: "AES-GCM", iv: bytes.slice(0, 12), additionalData: encoder.encode(aad) },
    await aesKey(env),
    bytes.slice(12),
  );
  return decoder.decode(pt);
}

export const serviceAad = () => "network_keys:1";
export const networkAad = (networkId) => `network_sign_keys:${networkId}`;

/** Rewrites plain rows sealed. Returns how many rows changed. No-op without the secret. */
export async function sealPlainRows(env) {
  if (!hasSealKey(env)) return 0;
  let changed = 0;
  const service = await env.DB.prepare("SELECT private_pkcs8 FROM network_keys WHERE id = 1").first();
  if (service && service.private_pkcs8 && !isSealed(service.private_pkcs8)) {
    const sealed = await sealKey(env, service.private_pkcs8, serviceAad());
    const r = await env.DB.prepare("UPDATE network_keys SET private_pkcs8 = ? WHERE id = 1 AND private_pkcs8 = ?")
      .bind(sealed, service.private_pkcs8)
      .run();
    changed += r?.meta?.changes ?? 0;
  }
  const { results = [] } = await env.DB.prepare(
    "SELECT network_id, private_pkcs8 FROM network_sign_keys WHERE private_pkcs8 NOT LIKE 'enc1:%' AND private_pkcs8 != ''",
  ).all();
  for (const row of results) {
    const sealed = await sealKey(env, row.private_pkcs8, networkAad(row.network_id));
    const r = await env.DB.prepare("UPDATE network_sign_keys SET private_pkcs8 = ? WHERE network_id = ? AND private_pkcs8 = ?")
      .bind(sealed, row.network_id, row.private_pkcs8)
      .run();
    changed += r?.meta?.changes ?? 0;
  }
  return changed;
}

let swept = false;

/** Runs sealPlainRows once per isolate, in the background. Never logs key material. */
export function sealPlainRowsOnce(env, ctx) {
  if (swept || !hasSealKey(env) || !env.DB) return;
  swept = true;
  const job = sealPlainRows(env)
    .then((n) => {
      if (n > 0) console.log(`sealed ${n} signing key row(s)`);
    })
    .catch((err) => {
      swept = false;
      console.error(`signing key seal sweep failed: ${err?.name || "error"}`);
    });
  if (ctx?.waitUntil) ctx.waitUntil(job);
}
