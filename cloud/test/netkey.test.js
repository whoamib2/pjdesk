import test from "node:test";
import assert from "node:assert/strict";
import { b64url, fromB64url, open, sealEmbeddedWith, sealPassphraseWith } from "../public/admin/netkey-src.mjs";

const contents = {
  networkId: "net_1",
  secretHex: "11".repeat(32),
  active: "office.example:21200",
  activeFingerprint: "sha256:AB-CD",
  standby: "cabin.example:21200",
  standbyFingerprint: "00112233",
};
const salt = new Uint8Array(16).fill(0x22);
const nonce = new Uint8Array(24).fill(0x33);
const key = new Uint8Array(32).fill(0x44);

const PASS =
  "pjdesk-net1:AQEiIiIiIiIiIiIiIiIiIiIiIAAAAAIAAAABAAAAMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzKOc4nuM-OveRpTDfJiHVaAyTTbmwtI0GPMF5rThrZ4SUnHdPSug_sIdPwyd6oaTP-Z_rMTTJ7bPn7zSdR43Sg806UBK7suIiEg8hBFnJ5xnojow6LryvM9RClVImNRq7Uv7DQDkfIn3SqQiwyHFT6H5MXb2SasEGO3nD6HdUmM-CEeMwDeZ6AAAzA5I9qWNMQ54vLg";
const EMBEDDED =
  "pjdesk-net1:AQJERERERERERERERERERERERERERERERERERERERERERDMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzM5JcfFHZpwBQRaG9g6f0eGoXUeLUeS61xXWt-uI1ycZSw82datp23gtlU1GdZHD-RGiUR0yNWepB_aH8oJQVwDOASUGPNShWH7F0Vqvtue_nYigyTpqI0V-LghRmeC-0bWVub1AHt3nVAyS4RVxgBGUQ1h_kvBuMMV9wDoXNBodr32qLeWJxV4t6xVHgZv4Ne4lf70E";

test("the blob matches the Rust vector and hides the address", async () => {
  const pass = await sealPassphraseWith(contents, "correct horse", salt, nonce, 32, 2, 1);
  assert.equal(pass, PASS);
  assert.equal(pass.includes("office.example"), false);
  const opened = await open(pass, "correct horse");
  assert.equal(opened.active, "office.example:21200");
  assert.equal(opened.activeFingerprint, "abcd");
  const embedded = await sealEmbeddedWith(contents, key, nonce);
  assert.equal(embedded, EMBEDDED);
  assert.equal((await open(embedded, "")).standby, "cabin.example:21200");
});

test("a wrong passphrase or a tampered blob does not open", async () => {
  await assert.rejects(() => open(PASS, "wrong passphrase"));
  const raw = fromB64url(PASS.slice("pjdesk-net1:".length));
  raw[raw.length - 1] ^= 0x01;
  await assert.rejects(() => open(`pjdesk-net1:${b64url(raw)}`, "correct horse"));
});
