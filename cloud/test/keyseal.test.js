import test from "node:test";
import assert from "node:assert/strict";
import { isSealed, networkAad, openKey, sealKey, sealPlainRows, serviceAad } from "../src/keyseal.js";

const env = { SIGNING_KEY_SECRET: "11".repeat(32) };
const other = { SIGNING_KEY_SECRET: "22".repeat(32) };

test("sealed keys round-trip and are bound to their row", async () => {
  const sealed = await sealKey(env, "ab".repeat(48), networkAad("net1"));
  assert.ok(isSealed(sealed));
  assert.ok(!sealed.includes("ab".repeat(8)));
  assert.equal(await openKey(env, sealed, networkAad("net1")), "ab".repeat(48));
  await assert.rejects(openKey(env, sealed, networkAad("net2")));
  await assert.rejects(openKey(other, sealed, networkAad("net1")));
  await assert.rejects(openKey({}, sealed, networkAad("net1")), /SIGNING_KEY_SECRET/);
});

test("plain rows from the older Worker still open, and nothing is sealed without the secret", async () => {
  assert.equal(await openKey(env, "cd".repeat(48), serviceAad()), "cd".repeat(48));
  assert.equal(await sealKey({}, "cd".repeat(48), serviceAad()), "cd".repeat(48));
  assert.equal(await sealKey({ SIGNING_KEY_SECRET: "short" }, "cd", serviceAad()), "cd");
});

function fakeDb(service, signRows) {
  return {
    prepare(sql) {
      const st = {
        args: [],
        bind(...a) { st.args = a; return st; },
        async first() { return sql.includes("network_keys") ? service : null; },
        async all() { return { results: signRows.filter((r) => !r.private_pkcs8.startsWith("enc1:") && r.private_pkcs8) }; },
        async run() {
          if (sql.startsWith("UPDATE network_keys")) { if (service.private_pkcs8 === st.args[1]) { service.private_pkcs8 = st.args[0]; return { meta: { changes: 1 } }; } }
          if (sql.startsWith("UPDATE network_sign_keys")) {
            const row = signRows.find((r) => r.network_id === st.args[1] && r.private_pkcs8 === st.args[2]);
            if (row) { row.private_pkcs8 = st.args[0]; return { meta: { changes: 1 } }; }
          }
          return { meta: { changes: 0 } };
        },
      };
      return st;
    },
  };
}

test("the sweep seals every plain row once and they still open", async () => {
  const service = { private_pkcs8: "aa".repeat(48) };
  const rows = [{ network_id: "n1", private_pkcs8: "bb".repeat(48) }, { network_id: "n2", private_pkcs8: "" }];
  const e = { ...env, DB: fakeDb(service, rows) };
  assert.equal(await sealPlainRows(e), 2);
  assert.ok(isSealed(service.private_pkcs8) && isSealed(rows[0].private_pkcs8));
  assert.equal(await openKey(e, service.private_pkcs8, serviceAad()), "aa".repeat(48));
  assert.equal(await openKey(e, rows[0].private_pkcs8, networkAad("n1")), "bb".repeat(48));
  assert.equal(await sealPlainRows(e), 0);
  assert.equal(await sealPlainRows({ DB: fakeDb({ private_pkcs8: "aa" }, []) }), 0);
});

test("a base64 or BOM-prefixed secret works; a short one seals nothing", async () => {
  const b64env = { SIGNING_KEY_SECRET: "\uFEFFq83vEjRWeJq83vEjRWeJq83vEjRWeJq83vEjRWeJq80=\r\n" };
  const sealed = await sealKey(b64env, "ab".repeat(48), serviceAad());
  assert.ok(isSealed(sealed));
  assert.equal(await openKey({ SIGNING_KEY_SECRET: "q83vEjRWeJq83vEjRWeJq83vEjRWeJq83vEjRWeJq80=" }, sealed, serviceAad()), "ab".repeat(48));
  assert.equal(await sealKey({ SIGNING_KEY_SECRET: "short" }, "ab".repeat(48), serviceAad()), "ab".repeat(48));
});
