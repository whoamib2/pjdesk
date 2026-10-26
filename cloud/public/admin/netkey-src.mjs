// Network key blob. The byte layout matches crates/pjdesk-proto/src/netkey.rs.
// The passphrase and the finished blob stay in the browser. They are not sent
// to pjdesk.com and this file does not log them.

import { xchacha20poly1305 } from "@noble/ciphers/chacha.js";
import { argon2id } from "@noble/hashes/argon2.js";

export const PREFIX = "pjdesk-net1:";
export const ARGON_MEM_KIB = 19456;
export const ARGON_ITERATIONS = 2;
export const ARGON_LANES = 1;

const MODE_PASSPHRASE = 1;
const MODE_EMBEDDED = 2;

function concat(parts) {
  const len = parts.reduce((n, p) => n + p.length, 0);
  const out = new Uint8Array(len);
  let i = 0;
  for (const p of parts) {
    out.set(p, i);
    i += p.length;
  }
  return out;
}

function u32(n) {
  const b = new Uint8Array(4);
  new DataView(b.buffer).setUint32(0, n, true);
  return b;
}

function readU32(b, i) {
  return new DataView(b.buffer, b.byteOffset, b.byteLength).getUint32(i, true);
}

export function b64url(bytes) {
  let s = "";
  for (let i = 0; i < bytes.length; i++) s += String.fromCharCode(bytes[i]);
  return btoa(s).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/g, "");
}

export function fromB64url(text) {
  const pad = text.length % 4 === 0 ? "" : "=".repeat(4 - (text.length % 4));
  const s = text.replace(/-/g, "+").replace(/_/g, "/") + pad;
  const bin = atob(s);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

function normalizeFingerprint(text) {
  const raw = String(text || "").trim().replace(/^sha256:/i, "");
  return raw.toLowerCase().replace(/[^0-9a-f]/g, "");
}

function canonical({ networkId, secretHex, active, activeFingerprint, standby, standbyFingerprint }) {
  const id = String(networkId || "").trim();
  const address = String(active || "").trim();
  const stand = String(standby || "").trim();
  if (!id || id.length > 80 || !address || address.length > 200 || stand.length > 200) {
    throw new Error("This network key is not valid.");
  }
  if (/[\n]/.test(id + address + stand)) throw new Error("This network key is not valid.");
  const secret = String(secretHex || "").trim().toLowerCase();
  if (!/^[0-9a-f]{64}$/.test(secret)) throw new Error("This network key is not valid.");
  const text = `PJNK1\n${id}\n${secret}\n${address}\n${normalizeFingerprint(activeFingerprint)}\n${stand}\n${normalizeFingerprint(standbyFingerprint)}\n`;
  return new TextEncoder().encode(text);
}

function parsePlain(bytes) {
  const text = new TextDecoder().decode(bytes);
  const lines = text.split("\n");
  if (lines[0] !== "PJNK1" || !lines[1] || !/^[0-9a-f]{64}$/.test(lines[2] || "") || !lines[3]) {
    throw new Error("This network key is not valid.");
  }
  return {
    networkId: lines[1],
    secretHex: lines[2],
    active: lines[3],
    activeFingerprint: lines[4] || "",
    standby: lines[5] || "",
    standbyFingerprint: lines[6] || "",
  };
}

function checkArgon(mem, iterations, lanes) {
  if (lanes < 1 || lanes > 4 || iterations < 1 || iterations > 8) throw new Error("This network key is not valid.");
  if (mem < 8 * lanes || mem > 64 * 1024) throw new Error("This network key is not valid.");
}

async function derive(passphrase, salt, mem, iterations, lanes) {
  checkArgon(mem, iterations, lanes);
  return argon2id(new TextEncoder().encode(passphrase), salt, {
    t: iterations,
    m: mem,
    p: lanes,
    dkLen: 32,
    version: 0x13,
  });
}

function sealBytes(key, nonce, aad, plain) {
  return xchacha20poly1305(key, nonce, aad).encrypt(plain);
}

function openBytes(key, nonce, aad, ct) {
  return xchacha20poly1305(key, nonce, aad).decrypt(ct);
}

export function randomBytes(n) {
  const b = new Uint8Array(n);
  crypto.getRandomValues(b);
  return b;
}

export async function sealPassphraseWith(contents, passphrase, salt, nonce, mem, iterations, lanes) {
  if (!passphrase) throw new Error("Enter a passphrase.");
  const key = await derive(passphrase, salt, mem, iterations, lanes);
  const plain = canonical(contents);
  const header = concat([new Uint8Array([1, MODE_PASSPHRASE]), salt, u32(mem), u32(iterations), u32(lanes)]);
  const ct = sealBytes(key, nonce, header, plain);
  key.fill(0);
  return PREFIX + b64url(concat([header, nonce, ct]));
}

export async function sealEmbeddedWith(contents, key, nonce) {
  const plain = canonical(contents);
  const header = concat([new Uint8Array([1, MODE_EMBEDDED]), key]);
  const ct = sealBytes(key, nonce, header, plain);
  return PREFIX + b64url(concat([header, nonce, ct]));
}

export async function sealPassphrase(contents, passphrase) {
  return sealPassphraseWith(contents, passphrase, randomBytes(16), randomBytes(24), ARGON_MEM_KIB, ARGON_ITERATIONS, ARGON_LANES);
}

export async function sealEmbedded(contents) {
  return sealEmbeddedWith(contents, randomBytes(32), randomBytes(24));
}

export function kind(blob) {
  const compact = String(blob || "").replace(/\s+/g, "");
  if (!compact.startsWith(PREFIX)) return "";
  try {
    const raw = fromB64url(compact.slice(PREFIX.length));
    if (raw[1] === MODE_PASSPHRASE) return "passphrase";
    if (raw[1] === MODE_EMBEDDED) return "embedded";
  } catch {
    return "";
  }
  return "";
}

export async function open(blob, passphrase) {
  const compact = String(blob || "").replace(/\s+/g, "");
  if (!compact.startsWith(PREFIX)) throw new Error("This network key is not valid.");
  const raw = fromB64url(compact.slice(PREFIX.length));
  if (raw[0] !== 1) throw new Error("This network key is not valid.");
  let key;
  let aadEnd;
  let nonceEnd;
  if (raw[1] === MODE_PASSPHRASE) {
    if (!passphrase || raw.length < 54 + 16) throw new Error("That network key could not be opened. Check the text and the passphrase.");
    const salt = raw.slice(2, 18);
    const mem = readU32(raw, 18);
    const iterations = readU32(raw, 22);
    const lanes = readU32(raw, 26);
    key = await derive(passphrase, salt, mem, iterations, lanes);
    aadEnd = 30;
    nonceEnd = 54;
  } else if (raw[1] === MODE_EMBEDDED) {
    if (raw.length < 58 + 16) throw new Error("This network key is not valid.");
    key = raw.slice(2, 34);
    aadEnd = 34;
    nonceEnd = 58;
  } else {
    throw new Error("This network key is not valid.");
  }
  const nonce = raw.slice(aadEnd, nonceEnd);
  const ct = raw.slice(nonceEnd);
  let plain;
  try {
    plain = openBytes(key, nonce, raw.slice(0, aadEnd), ct);
  } catch {
    throw new Error("That network key could not be opened. Check the text and the passphrase.");
  } finally {
    key.fill(0);
  }
  return parsePlain(plain);
}

export async function secretHash(secretHex) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(secretHex.trim().toLowerCase()));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}
