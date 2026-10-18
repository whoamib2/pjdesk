(() => {
  const $ = (id) => document.getElementById(id);
  let creating = false;
  PJ.revealToggles();

  function setMode(create) {
    creating = create;
    $("tab-in").classList.toggle("on", !create);
    $("tab-up").classList.toggle("on", create);
    PJ.show($("confirm-wrap"), create);
    $("auth-submit").textContent = create ? "Create account" : "Sign in";
    $("password").autocomplete = create ? "new-password" : "current-password";
    PJ.message($("auth-msg"), "");
  }
  $("tab-in").onclick = () => setMode(false);
  $("tab-up").onclick = () => setMode(true);

  // Checked here, before the password is turned into an auth key. The Worker
  // never sees the password, so it cannot apply this itself. Sign-in is not
  // checked: an existing account must keep working.
  function passwordProblem(pw, email) {
    if (pw !== pw.trim()) return "Don't start or end the password with a space.";
    if (pw.length < 12) return "Use at least 12 characters.";
    if (!/[a-z]/.test(pw) || !/[A-Z]/.test(pw) || !/[0-9]/.test(pw) || !/[^A-Za-z0-9\s]/.test(pw)) {
      return "Use an uppercase letter, a lowercase letter, a digit, and a symbol.";
    }
    const local = (email.split("@")[0] || "").toLowerCase();
    if (local.length >= 3 && pw.toLowerCase().includes(local)) return "Don't include your email name in the password.";
    const common = ["password", "qwerty", "letmein", "welcome", "admin", "iloveyou", "monkey", "dragon", "master", "login", "abc123", "passw0rd", "p@ssw0rd"];
    const fold = pw.toLowerCase().replace(/\s/g, "");
    if (common.some((w) => fold === w || (w.length >= 6 && fold.includes(w)))) return "That password is too common.";
    return "";
  }

  $("auth-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const email = $("email").value.trim();
    const pw = $("password").value;
    const msg = $("auth-msg");
    if (!email || !pw) return PJ.message(msg, "Enter your email and password.", "err");
    if (creating) {
      const problem = passwordProblem(pw, email);
      if (problem) return PJ.message(msg, problem, "err");
      if (pw !== $("password2").value) return PJ.message(msg, "The passwords don't match.", "err");
    }
    const btn = $("auth-submit");
    btn.disabled = true;
    PJ.message(msg, creating ? "Creating your account…" : "Signing in…");
    try {
      const auth_key = await PJ.authKey(email, pw);
      if (creating) await PJ.call("POST", "/api/signup", { email, auth_key });
      const r = await PJ.call("POST", "/api/login", { email, auth_key, device: "Web browser" });
      PJ.setToken(r.token);
      $("password").value = $("password2").value = "";
      await load();
    } catch (e) {
      PJ.message(msg, e.message, "err");
    } finally {
      btn.disabled = false;
    }
  });

  $("pw-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const msg = $("pw-msg");
    const oldPw = $("old-pw").value, newPw = $("new-pw").value;
    const problem = passwordProblem(newPw, $("me-email").dataset.email || "");
    if (problem) return PJ.message(msg, problem, "err");
    if (newPw !== $("new-pw2").value) return PJ.message(msg, "The new passwords don't match.", "err");
    PJ.message(msg, "Changing…");
    try {
      const email = $("me-email").dataset.email;
      const [old_auth_key, new_auth_key] = await Promise.all([PJ.authKey(email, oldPw), PJ.authKey(email, newPw)]);
      await PJ.call("POST", "/api/password", { old_auth_key, new_auth_key });
      $("old-pw").value = $("new-pw").value = $("new-pw2").value = "";
      PJ.message(msg, "Password changed. Sign in again on your PJDesk servers, starting with the main one.", "ok");
      load();
    } catch (e) {
      PJ.message(msg, e.message, "err");
    }
  });

  $("sign-out").onclick = async () => {
    try { await PJ.call("POST", "/api/logout"); } catch { /* already signed out */ }
    PJ.setToken(null);
    load();
  };

  function syncPill(on) {
    const pill = document.createElement("span");
    pill.className = on ? "pill on" : "pill off";
    pill.textContent = on ? "● On" : "Not turned on yet";
    $("sync-status").replaceChildren(pill);
  }

  async function load() {
    if (!PJ.token()) {
      PJ.show($("signed-in"), false);
      PJ.show($("signed-out"), true);
      return;
    }
    try {
      const me = await PJ.call("GET", "/api/me");
      $("me-email").textContent = "Signed in as " + me.email;
      $("me-email").dataset.email = me.email;
      syncPill(!!me.sync_enabled);
      $("sync-last").textContent = me.sync ? PJ.date(me.sync.updated_at) : "No backup yet";
      $("sync-size").textContent = me.sync ? PJ.size(me.sync.size) : "—";
      PJ.show($("sync-howto"), me.sync_enabled);
      const rows = (me.devices || []).map((d) => {
        const tr = document.createElement("tr");
        for (const v of [d.device || "Unknown", PJ.date(d.created_at), PJ.date(d.last_used_at)]) {
          const td = document.createElement("td");
          td.textContent = v;
          tr.appendChild(td);
        }
        return tr;
      });
      $("devices").replaceChildren(...rows);
      PJ.show($("admin-link"), me.is_admin);
      PJ.show($("signed-out"), false);
      PJ.show($("signed-in"), true);
    } catch (e) {
      if (e.status === 401) {
        PJ.setToken(null);
        return load();
      }
      PJ.show($("signed-out"), true);
      PJ.message($("auth-msg"), e.message, "err");
    }
  }

  if (location.hash === "#create") setMode(true);
  load();
})();
