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

  function svg(path) {
    const ns = "http://www.w3.org/2000/svg";
    const root = document.createElementNS(ns, "svg");
    root.setAttribute("viewBox", "0 0 24 24");
    const p = document.createElementNS(ns, "path");
    p.setAttribute("d", path);
    root.append(p);
    return root;
  }

  function statCard(label, value, delta, icon) {
    const card = el("div");
    card.className = "card";
    const top = el("div");
    top.className = "card-top";
    const text = el("div");
    text.append(el("div", label, "stat-label"), el("div", String(value), "stat"));
    const mark = el("div");
    mark.className = "ico";
    mark.append(svg(icon));
    top.append(text, mark);
    card.append(top);
    if (delta) card.append(el("div", delta, "delta"));
    return card;
  }

  function overviewModel() {
    const week = Math.floor(Date.now() / 1000) - 7 * 86400;
    const accounts = fixture.accounts.map((a, i) => (i === 2 ? { ...a, created_at: week + 86400 } : a));
    return {
      accounts: accounts.length,
      cloud: accounts.filter((a) => a.plan === "cloud" || a.is_admin).length,
      free: accounts.filter((a) => a.plan !== "cloud" && !a.is_admin).length,
      new_accounts_7d: accounts.filter((a) => a.created_at >= week).length,
      servers: fixture.servers.servers.length,
      signins: fixture.servers.signins.length,
      active_today: 0,
      audit: fixture.audit.length,
      server_list: fixture.servers.servers,
      recent_audit: fixture.audit,
      recent_signins: fixture.servers.signins,
    };
  }

  async function renderOverview() {
    const data = preview ? overviewModel() : await loadJson("/api/admin/overview", fixture.overview);
    const latest = preview ? fixture.latest : await latestVersion();
    const icons = {
      accounts: "M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2M9 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8M22 21v-2a4 4 0 0 0-3-3.87M16 3.13a4 4 0 0 1 0 7.75",
      cloud: "M20 17.5A4.5 4.5 0 0 0 18 9h-1.3A6 6 0 1 0 6 14.5",
      free: "M12 3l2.2 4.6L19 8.3l-3.5 3.4.8 4.8L12 14.8 7.7 16.5l.8-4.8L5 8.3l4.8-.7L12 3z",
      servers: "M4 6h16v4H4zM4 14h16v4H4zM7 8h.01M7 16h.01",
    };
    const wrap = el("div");
    wrap.className = "cards";
    const fresh = Number(data.new_accounts_7d) || 0;
    const active = Number(data.active_today) || 0;
    wrap.append(
      statCard("Accounts", data.accounts, fresh ? fresh + " new this week" : "", icons.accounts),
      statCard("Cloud Sync", data.cloud, "", icons.cloud),
      statCard("Free", data.free, "", icons.free),
      statCard("Servers", data.servers, active ? active + " sign-ins today" : "", icons.servers),
    );

    const actions = el("div", null, "panel");
    actions.append(el("h2", "Quick actions"));
    const row = el("div", null, "actions");
    const grant = el("button", "Grant plan", "primary");
    grant.type = "button";
    grant.addEventListener("click", () => show("accounts"));
    const view = el("button", "View accounts", "ghost");
    view.type = "button";
    view.addEventListener("click", () => show("accounts"));
    row.append(grant, view);
    actions.append(row);

    const servers = el("div", null, "panel");
    servers.append(el("h2", "Servers"));
    const list = data.server_list || [];
    if (!list.length) {
      servers.append(el("p", "No server has reported a health record yet.", "empty"));
    } else {
      const table = document.createElement("table");
      table.append(rowOf(["Name", "Role", "Version", "Last seen"], true));
      for (const s of list) {
        const ver = el("td");
        ver.append(document.createTextNode(s.version || "unknown"));
        if (versionBehind(s.version, latest)) ver.append(el("span", " update available", "badge warn"));
        const tr = document.createElement("tr");
        tr.append(el("td", s.name || "—"), el("td", s.role || "—"), ver, el("td", date(s.reported_at), "muted"));
        table.append(tr);
      }
      servers.append(table);
    }

    const side = el("div");
    const plans = el("div", null, "panel");
    plans.append(el("h2", "Plans"));
    const total = Math.max(1, Number(data.cloud) + Number(data.free));
    const bar = el("div", null, "split");
    const cloud = el("span");
    cloud.className = "cloud";
    cloud.style.width = Math.round((Number(data.cloud) / total) * 100) + "%";
    const free = el("span");
    free.className = "free";
    free.style.width = Math.round((Number(data.free) / total) * 100) + "%";
    bar.append(cloud, free);
    const legend = el("div", null, "legend");
    legend.append(el("span", "Cloud Sync " + data.cloud), el("span", "Free " + data.free));
    plans.append(bar, legend);

    const activity = el("div", null, "panel");
    activity.append(el("h2", "Recent activity"));
    const events = [];
    for (const a of data.recent_audit || []) {
      events.push({ at: a.at, text: (a.actor || "Someone") + " set " + a.email + " from " + (a.old_plan || "free") + " to " + (a.new_plan || "free") });
    }
    for (const s of data.recent_signins || []) {
      events.push({ at: s.last_used_at, text: (s.email || "Account") + " signed in" + (s.device ? " · " + s.device : "") });
    }
    events.sort((a, b) => (b.at || 0) - (a.at || 0));
    const feed = el("div", null, "activity");
    if (!events.length) feed.append(el("p", "No plan changes or sign-ins yet.", "empty"));
    else {
      for (const ev of events.slice(0, 6)) {
        const item = el("div", null, "item");
        item.append(el("span", ev.text), el("span", date(ev.at), "muted"));
        feed.append(item);
      }
    }
    activity.append(feed);
    side.append(plans, activity);

    const dash = el("div", null, "dash");
    dash.append(servers, side);
    $("overview").replaceChildren(wrap, actions, dash);
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
  if (params.get("theme") === "light" || localStorage.getItem("pjdesk.admin.theme") === "light") {
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
