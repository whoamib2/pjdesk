fetch("/api/release")
  .then((r) => (r.ok ? r.json() : null))
  .then((r) => {
    if (r && typeof r.version === "string" && /^[0-9]+(\.[0-9]+){0,3}$/.test(r.version)) {
      document.getElementById("dl-version").textContent = "Version " + r.version;
    }
  })
  .catch(() => {});

async function api(path, opts = {}) {
  const init = { method: opts.method || "GET", credentials: "same-origin", headers: {} };
  if (opts.body !== undefined) {
    init.headers["content-type"] = "application/json";
    init.body = JSON.stringify(opts.body);
  }
  let r;
  try {
    r = await fetch(path, init);
  } catch {
    throw Object.assign(new Error("Can't reach pjdesk.com. Check your connection and try again."), { code: "network" });
  }
  let data = {};
  try {
    data = await r.json();
  } catch {
    /* empty body */
  }
  if (!r.ok) throw Object.assign(new Error(data.error || `Something went wrong (HTTP ${r.status}).`), { status: r.status, code: data.code, data });
  return data;
}

function show(el, on) {
  if (el) el.hidden = !on;
}

async function loadDownload() {
  const form = document.getElementById("unlock-sec");
  const files = document.getElementById("dl-files");
  const note = document.getElementById("dl-note");
  const closed = document.getElementById("dl-closed");
  if (!form || !files) return;
  try {
    const st = await api("/api/download/state");
    if (!st.password_set) {
      show(form, false);
      show(files, false);
      show(note, false);
      show(closed, true);
      return;
    }
    show(closed, false);
    show(form, !st.unlocked);
    show(files, !!st.unlocked);
    show(note, !!st.unlocked);
    if (!st.unlocked) {
      const input = document.querySelector("#unlock input[name=password]");
      if (input) input.focus();
    }
  } catch {
    show(form, true);
    show(files, false);
    show(note, false);
  }
}

const unlock = document.getElementById("unlock");
if (unlock) {
  const err = unlock.querySelector(".error");
  unlock.addEventListener("submit", async (e) => {
    e.preventDefault();
    const btn = unlock.querySelector("button[type=submit]");
    if (err) err.textContent = "";
    if (btn) btn.disabled = true;
    try {
      const v = Object.fromEntries(new FormData(unlock));
      await api("/api/download/unlock", { method: "POST", body: { password: v.password || "" } });
      unlock.reset();
      await loadDownload();
    } catch (ex) {
      if (err) err.textContent = ex.message || String(ex);
    } finally {
      if (btn) btn.disabled = false;
    }
  });
  const input = unlock.querySelector("input[name=password]");
  const reveal = unlock.querySelector(".reveal");
  if (input && reveal) {
    reveal.addEventListener("click", () => {
      const showPw = input.type === "password";
      input.type = showPw ? "text" : "password";
      reveal.textContent = showPw ? "Hide" : "Show";
      reveal.setAttribute("aria-label", showPw ? "Hide password" : "Show password");
      input.focus();
    });
  }
  loadDownload();
}
