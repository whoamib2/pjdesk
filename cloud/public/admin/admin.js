(() => {
  const $ = (id) => document.getElementById(id);
  const params = new URLSearchParams(location.search);
  const preview = params.get("preview") === "1";
  const titles = {
    overview: ["Overview", "Accounts, Cloud Sync, and the servers that last reported in."],
    accounts: ["Accounts", "Grant Free or Cloud Sync. Administrators always have Cloud Sync."],
    servers: ["Servers & devices", "Servers that published a health record, plus account sign-ins."],
    plans: ["Plans", "Free is the default. Cloud Sync is the only paid-style switch the site offers. There is no price list."],
    audit: ["Audit log", "Who changed a plan, and what it was before."],
  };

  const fixture = {
    overview: { accounts: 3, cloud: 2, free: 1, servers: 2, signins: 2, audit: 2 },
    accounts: [
      { email: "ben@spader.us", created_at: 1700000000, last_login_at: 1729000000, signed_in: 1, synced_at: 1729001000, size: 24000, is_admin: true, plan: "cloud", plan_expires_at: null, plan_note: "", sync_enabled: true },
      { email: "ada@example.com", created_at: 1710000000, last_login_at: 1728000000, signed_in: 1, synced_at: 1728001000, size: 12000, is_admin: false, plan: "cloud", plan_expires_at: 1893456000, plan_note: "Annual", sync_enabled: true },
      { email: "guest@example.com", created_at: 1720000000, last_login_at: null, signed_in: 0, synced_at: null, size: null, is_admin: false, plan: "free", plan_expires_at: null, plan_note: "", sync_enabled: false },
    ],
    servers: {
      note: "Desktop client versions are on each PJDesk Server window. This page lists servers that reported a Cloud Sync health record, and account sign-ins. Sign-ins do not include a client version.",
      servers: [
        { email: "ben@spader.us", name: "office", version: "0.5.2", role: "active", healthy: 1, reported_at: 1729000000 },
        { email: "ben@spader.us", name: "cabin", version: "0.5.1", role: "standby", healthy: 1, reported_at: 1728900000 },
      ],
      signins: [
        { email: "ben@spader.us", device: "Office browser", created_at: 1720000000, last_used_at: 1729000000 },
        { email: "ada@example.com", device: "Home", created_at: 1715000000, last_used_at: 1728000000 },
      ],
    },
    audit: [
      { at: 1728000000, actor: "ben@spader.us", email: "ada@example.com", old_plan: "free", new_plan: "cloud", expires_at: 1893456000, note: "Annual" },
      { at: 1727000000, actor: "expiry", email: "old@example.com", old_plan: "cloud", new_plan: "free", expires_at: 1727000000, note: "Cloud Sync grant expired" },
    ],
    latest: "0.5.2",
  };

  let accounts = [];
  let accountQuery = "";
  let sortKey = "email";
  let sortDir = 1;
  let grantEmail = "";
  let pending = null;

  function date(unix) {
    if (!unix) return "—";
    if (window.PJ && PJ.date) return PJ.date(unix);
    return new Date(unix * 1000).toLocaleString();
  }
  function size(n) {
    if (!n) return "—";
    if (window.PJ && PJ.size) return PJ.size(n);
    return `${n} B`;
  }
  function badge(plan, admin) {
    const span = document.createElement("span");
    span.className = "badge" + (plan === "cloud" ? " cloud" : "");
    span.textContent = admin ? "Admin" : plan === "cloud" ? "Cloud Sync" : "Free";
    return span;
  }
  function el(tag, text, cls) {
    const n = document.createElement(tag);
    if (text != null) n.textContent = text;
    if (cls) n.className = cls;
    return n;
  }

  function show(page) {
    document.querySelectorAll("nav button").forEach((b) => b.classList.toggle("on", b.dataset.page === page));
    document.querySelectorAll("main .page").forEach((s) => s.classList.toggle("hidden", s.id !== page));
    $("title").textContent = titles[page][0];
    $("sub").textContent = titles[page][1];
    if (page === "overview") renderOverview();
    if (page === "accounts") renderAccounts();
    if (page === "servers") renderServers();
    if (page === "plans") renderPlans();
    if (page === "audit") renderAudit();
  }

  async function loadJson(path, fallback) {
    if (preview) return fallback;
    return PJ.call("GET", path);
  }

  async function renderOverview() {
    const data = await loadJson("/api/admin/overview", fixture.overview);
    const cards = [
      ["Accounts", data.accounts],
      ["Cloud Sync", data.cloud],
      ["Free", data.free],
      ["Servers", data.servers],
    ];
    const wrap = el("div");
    wrap.className = "cards";
    for (const [label, n] of cards) {
      const c = el("div");
      c.className = "card";
      c.append(el("div", label, "stat-label"), el("div", String(n), "stat"));
      wrap.append(c);
    }
    const note = el("div", "Sign-ins " + data.signins + " · Audit rows " + data.audit, "panel note");
    $("overview").replaceChildren(wrap, note);
  }

  function renderAccounts() {
    const q = accountQuery;
    const rows = accounts
      .filter((a) => !q || a.email.includes(q))
      .slice()
      .sort((a, b) => {
        const av = a[sortKey] ?? "";
        const bv = b[sortKey] ?? "";
        if (av < bv) return -sortDir;
        if (av > bv) return sortDir;
        return 0;
      });
    const panel = el("div");
    panel.className = "panel";
    const bar = el("div");
    bar.className = "toolbar";
    const search = document.createElement("input");
    search.id = "acct-filter";
    search.type = "search";
    search.placeholder = "Search email";
    search.value = q;
    search.addEventListener("input", () => {
      accountQuery = search.value.trim().toLowerCase();
      renderAccounts();
    });
    bar.append(search, el("span", `${rows.length} of ${accounts.length}`, "muted"));
    const table = document.createElement("table");
    const head = document.createElement("tr");
    for (const [key, label] of [["email", "Email"], ["plan", "Plan"], ["plan_expires_at", "Expires"], ["signed_in", "Signed in"], ["last_login_at", "Last sign-in"]]) {
      const th = document.createElement("th");
      const btn = document.createElement("button");
      btn.className = "ghost";
      btn.textContent = label;
      btn.addEventListener("click", () => {
        if (sortKey === key) sortDir *= -1;
        else { sortKey = key; sortDir = 1; }
        renderAccounts();
      });
      th.append(btn);
      head.append(th);
    }
    head.append(el("th", ""));
    table.append(head);
    for (const a of rows) {
      const tr = document.createElement("tr");
      const email = el("td");
      email.append(document.createTextNode(a.email), document.createTextNode(" "), badge(a.plan, a.is_admin));
      const plan = el("td");
      plan.append(badge(a.plan, false));
      if (a.plan_note) plan.append(el("div", a.plan_note, "muted"));
      const grant = document.createElement("button");
      grant.className = "primary";
      grant.textContent = "Grant plan";
      grant.addEventListener("click", () => openGrant(a));
      const actions = el("td");
      actions.append(grant);
      tr.append(email, plan, el("td", a.plan_expires_at ? date(a.plan_expires_at) : "—", "muted"), el("td", String(a.signed_in || 0)), el("td", date(a.last_login_at), "muted"), actions);
      table.append(tr);
    }
    panel.append(bar, table);
    $("accounts").replaceChildren(panel);
  }

  function versionBehind(reported, latest) {
    if (!latest) return false;
    if (!reported) return true;
    const parts = (s) => String(s).replace(/^v/, "").split(".").map((n) => parseInt(n, 10) || 0);
    const a = parts(reported);
    const b = parts(latest);
    for (let i = 0; i < Math.max(a.length, b.length); i++) {
      const av = a[i] || 0;
      const bv = b[i] || 0;
      if (av < bv) return true;
      if (av > bv) return false;
    }
    return false;
  }

  async function latestVersion() {
    if (preview) return fixture.latest || "";
    try {
      const rel = await fetch("/api/release").then((r) => r.json());
      return rel && rel.version ? String(rel.version) : "";
    } catch {
      return "";
    }
  }

  async function renderServers() {
    const data = await loadJson("/api/admin/servers", fixture.servers);
    const latest = await latestVersion();
    const panel = el("div");
    panel.className = "panel";
    panel.append(el("p", data.note, "note"));
    panel.append(el("h2", "Servers"));
    const table = document.createElement("table");
    table.append(rowOf(["Account", "Name", "Version", "Role", "Last report"], true));
    for (const s of data.servers) {
      const ver = el("td");
      ver.append(document.createTextNode(s.version || "unknown"));
      if (versionBehind(s.version, latest)) {
        const b = el("span", " update available", "badge warn");
        ver.append(b);
      }
      const tr = document.createElement("tr");
      tr.append(el("td", s.email), el("td", s.name || "—"), ver, el("td", s.role || "—"), el("td", date(s.reported_at), "muted"));
      table.append(tr);
    }
    panel.append(table);
    panel.append(el("h2", "Sign-ins"));
    const people = document.createElement("table");
    people.append(rowOf(["Account", "Where", "Signed in", "Last active"], true));
    for (const s of data.signins) {
      const tr = document.createElement("tr");
      tr.append(el("td", s.email), el("td", s.device || "—"), el("td", date(s.created_at), "muted"), el("td", date(s.last_used_at), "muted"));
      people.append(tr);
    }
    panel.append(people);
    $("servers").replaceChildren(panel);
  }

  function renderPlans() {
    const panel = el("div");
    panel.className = "panel";
    const table = document.createElement("table");
    table.append(rowOf(["Plan", "What it does"], true));
    for (const [name, what] of [
      ["Free", "Cloud Sync off. This is the default for a new account."],
      ["Cloud Sync", "The account can keep an encrypted server copy and a standby server. The site does not list a price."],
    ]) {
      const tr = document.createElement("tr");
      tr.append(el("td", name), el("td", what));
      table.append(tr);
    }
    panel.append(
      el("p", "Granting a plan is an admin action on the Accounts page. A local PJDesk Server user account is not a pjdesk.com plan.", "note"),
      table,
    );
    $("plans").replaceChildren(panel);
  }

  async function renderAudit() {
    const data = await loadJson("/api/admin/audit", { audit: fixture.audit });
    const panel = el("div");
    panel.className = "panel";
    const table = document.createElement("table");
    table.append(rowOf(["When", "Who", "Account", "Change", "Expires", "Note"], true));
    for (const r of data.audit) {
      const tr = document.createElement("tr");
      tr.append(
        el("td", date(r.at), "muted"),
        el("td", r.actor),
        el("td", r.email),
        el("td", `${r.old_plan} → ${r.new_plan}`),
        el("td", r.expires_at ? date(r.expires_at) : "—", "muted"),
        el("td", r.note || "—"),
      );
      table.append(tr);
    }
    panel.append(table);
    $("audit").replaceChildren(panel);
  }

  function rowOf(labels, head) {
    const tr = document.createElement("tr");
    for (const label of labels) tr.append(el(head ? "th" : "td", label));
    return tr;
  }

  function openGrant(account) {
    grantEmail = account.email;
    $("grant-who").textContent = account.email + (account.is_admin ? " · administrator (Cloud Sync stays on)" : "");
    $("grant-plan").value = account.plan === "cloud" ? "cloud" : "free";
    $("grant-note").value = account.plan_note || "";
    $("grant-expires").value = "";
    $("grant-msg").textContent = "";
    $("grant").classList.remove("hidden");
  }

  function closeGrant() { $("grant").classList.add("hidden"); }

  async function saveGrant(ev) {
    ev.preventDefault();
    const plan = $("grant-plan").value;
    const current = accounts.find((a) => a.email === grantEmail);
    const downgrade = current && current.plan === "cloud" && plan === "free";
    const expiresRaw = $("grant-expires").value;
    const expires_at = expiresRaw ? Math.floor(new Date(expiresRaw + "T23:59:59").getTime() / 1000) : null;
    const body = { email: grantEmail, plan, expires_at, note: $("grant-note").value.trim() };
    if (downgrade) {
      pending = body;
      $("confirm-text").textContent = `Turn Cloud Sync off for ${grantEmail}? The account goes back to Free.`;
      $("confirm").classList.remove("hidden");
      return;
    }
    await sendGrant(body);
  }

  async function sendGrant(body) {
    if (preview) {
      const row = accounts.find((a) => a.email === body.email);
      if (row) {
        fixture.audit.unshift({ at: Math.floor(Date.now() / 1000), actor: "ben@spader.us", email: body.email, old_plan: row.plan, new_plan: body.plan, expires_at: body.expires_at, note: body.note });
        row.plan = body.plan;
        row.sync_enabled = body.plan === "cloud";
        row.plan_note = body.note;
        row.plan_expires_at = body.expires_at;
      }
      closeGrant();
      $("confirm").classList.add("hidden");
      renderAccounts();
      return;
    }
    try {
      await PJ.call("POST", "/api/admin/plan", body);
      closeGrant();
      $("confirm").classList.add("hidden");
      accounts = (await PJ.call("GET", "/api/admin/accounts")).accounts;
      renderAccounts();
    } catch (e) {
      $("grant-msg").textContent = e.message;
      $("confirm").classList.add("hidden");
    }
  }

  document.querySelectorAll("nav button").forEach((b) => b.addEventListener("click", () => show(b.dataset.page)));
  $("theme").addEventListener("click", () => {
    const light = document.body.classList.toggle("light");
    document.body.classList.toggle("dark", !light);
    $("theme").textContent = light ? "Dark" : "Light";
    localStorage.setItem("pjdesk.admin.theme", light ? "light" : "dark");
  });
  if (localStorage.getItem("pjdesk.admin.theme") === "light") {
    document.body.classList.add("light");
    document.body.classList.remove("dark");
    $("theme").textContent = "Dark";
  }
  $("grant-cancel").addEventListener("click", closeGrant);
  $("grant").querySelector("form").addEventListener("submit", saveGrant);
  $("confirm-no").addEventListener("click", () => $("confirm").classList.add("hidden"));
  $("confirm").querySelector("form").addEventListener("submit", (ev) => {
    ev.preventDefault();
    if (pending) sendGrant(pending);
  });

  async function boot() {
    if (!preview && !PJ.token()) { location.href = "/account/"; return; }
    try {
      if (preview) accounts = fixture.accounts;
      else accounts = (await PJ.call("GET", "/api/admin/accounts")).accounts;
      const page = params.get("page") || "overview";
      show(page);
      if (params.get("grant") === "1" && accounts[1]) openGrant(accounts[1]);
    } catch (e) {
      if (e.status === 401) { PJ.setToken(null); location.href = "/account/"; return; }
      $("overview").replaceChildren(el("p", e.message, "msg"));
    }
  }
  boot();
})();
