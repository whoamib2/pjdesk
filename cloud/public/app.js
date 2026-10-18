// Shared helpers for the pjdesk.com account pages.
//
// The password never leaves this page. It is stretched with PBKDF2 (600,000
// rounds, salted with the email) and only an "auth key" derived from it is sent
// to PJDesk Cloud. PJDesk Server derives the same key the same way, plus a
// separate encryption key that never leaves the server.
const PJ = (() => {
  const enc = new TextEncoder();
  const toHex = (buf) => [...new Uint8Array(buf)].map((b) => b.toString(16).padStart(2, "0")).join("");

  async function master(email, password) {
    const e = String(email).trim().toLowerCase();
    const salt = await crypto.subtle.digest("SHA-256", enc.encode("pjdesk-cloud-v1\0" + e));
    const base = await crypto.subtle.importKey("raw", enc.encode(password), "PBKDF2", false, ["deriveBits"]);
    return crypto.subtle.deriveBits({ name: "PBKDF2", hash: "SHA-256", salt, iterations: 600000 }, base, 256);
  }

  /** The key that proves the password to PJDesk Cloud (hex). */
  async function authKey(email, password) {
    const key = await crypto.subtle.importKey("raw", await master(email, password), { name: "HMAC", hash: "SHA-256" }, false, ["sign"]);
    return toHex(await crypto.subtle.sign("HMAC", key, enc.encode("pjdesk-cloud-auth")));
  }

  const TOKEN = "pjdesk.cloud.token";
  function token() {
    try { return localStorage.getItem(TOKEN); } catch { return null; }
  }
  function setToken(t) {
    try { t ? localStorage.setItem(TOKEN, t) : localStorage.removeItem(TOKEN); } catch { /* private window */ }
  }

  async function call(method, path, body) {
    const headers = { "content-type": "application/json" };
    const t = token();
    if (t) headers.authorization = "Bearer " + t;
    const res = await fetch(path, { method, headers, body: body ? JSON.stringify(body) : undefined });
    let data = {};
    try { data = await res.json(); } catch { /* empty */ }
    if (!res.ok) {
      const err = new Error(data.error || `Request failed (HTTP ${res.status})`);
      err.status = res.status;
      throw err;
    }
    return data;
  }

  function date(unix) {
    if (!unix) return "—";
    return new Date(unix * 1000).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
  }
  function size(n) {
    if (n == null) return "—";
    if (n < 1024) return n + " B";
    if (n < 1024 * 1024) return (n / 1024).toFixed(1) + " KB";
    return (n / 1024 / 1024).toFixed(1) + " MB";
  }

  /** Wires every button with data-reveal="<input id>" as a Show/Hide toggle. */
  function revealToggles(root = document) {
    root.querySelectorAll("[data-reveal]").forEach((btn) => {
      btn.addEventListener("click", () => {
        const input = document.getElementById(btn.dataset.reveal);
        const show = input.type === "password";
        input.type = show ? "text" : "password";
        btn.textContent = show ? "Hide" : "Show";
      });
    });
  }

  function show(el, on) { el.classList.toggle("hidden", !on); }
  function message(el, text, kind) {
    el.textContent = text || "";
    el.className = "msg" + (kind ? " " + kind : "");
  }

  return { authKey, token, setToken, call, date, size, revealToggles, show, message };
})();
