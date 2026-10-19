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
    network: ["My network", "The private server this account signs in to, and who else may use it."],
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
    network: {
      role: "owner",
      note: "Computers that have not signed in are listed in the PJDesk Server window until you turn on Require signed-in accounts.",
      network: {
        id: "net_office",
        name: "Office",
        active_address: "office.example:21200",
        active_fingerprint: "ab12cd34",
        standby_address: "cabin.example:21200",
        standby_fingerprint: "ef56ab78",
        require_accounts: false,
      },
      members: [
        { email: "ada@example.com", created_at: 1720000000, revoked_at: null },
      ],
      devices: [
        { device_id: "office-pc", email: "ben@spader.us", name: "Office PC", enrolled_at: 1728000000, revoked_at: null },
        { device_id: "old-phone", email: "ada@example.com", name: "Old phone", enrolled_at: 1727000000, revoked_at: 1728500000 },
      ],
      computers: [
        { id: "123456789", name: "Office", os_family: "windows", os_version: "11" },
        { id: "555000111", name: "Pixel", os_family: "android", os_version: "15" },
      ],
      invites: [
        {
          id: "ink_office01",
          mode: "passphrase",
          bound_email: "",
          max_uses: 5,
          uses: 1,
          expires_at: null,
          created_at: 1728000000,
          revoked_at: null,
          history: [{ device_id: "laptop-1", device_name: "Laptop", email: "ben@spader.us", used_at: 1728100000 }],
        },
      ],
    },
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
    if (page === "network") renderNetwork();
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

  function field(label, id, value) {
    const wrap = el("label", label);
    const input = document.createElement("input");
    input.id = id;
    input.value = value || "";
    wrap.append(input);
    return wrap;
  }

  async function renderNetwork() {
    const root = $("network");
    root.replaceChildren(el("p", "Loading…", "muted"));
    let data;
    try {
      data = preview ? fixture.network : await PJ.call("GET", "/api/network");
    } catch (e) {
      root.replaceChildren(el("p", e.message || "Could not load your network.", "msg"));
      return;
    }
    root.replaceChildren();
    const net = (data && data.network) || {
      name: "",
      active_address: "",
      active_fingerprint: "",
      standby_address: "",
      standby_fingerprint: "",
      require_accounts: false,
    };
    const form = el("form", null, "panel");
    form.append(el("h2", "Servers"));
    form.append(el("p", "Clients get this address and key when they sign in. Nothing is typed by hand on the computer.", "muted"));
    form.append(field("Name", "net-name", net.name));
    form.append(field("Active server (host or host:port)", "net-active", net.active_address));
    form.append(field("Active server key fingerprint", "net-active-fp", net.active_fingerprint));
    form.append(field("Standby server", "net-standby", net.standby_address));
    form.append(field("Standby server key fingerprint", "net-standby-fp", net.standby_fingerprint));
    const requireRow = el("label", null, "row");
    const box = document.createElement("input");
    box.type = "checkbox";
    box.id = "net-require";
    box.checked = !!net.require_accounts;
    requireRow.append(box, document.createTextNode(" Require signed-in accounts"));
    form.append(requireRow);
    form.append(el("p", data.note || "Turn this on only after every computer has signed in. Until then, computers that already registered keep working.", "muted"));
    const save = el("button", "Save network", "primary");
    save.type = "submit";
    form.append(save);
    const msg = el("p", "", "msg");
    form.append(msg);
    form.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const body = {
        name: $("net-name").value,
        active_address: $("net-active").value,
        active_fingerprint: $("net-active-fp").value,
        standby_address: $("net-standby").value,
        standby_fingerprint: $("net-standby-fp").value,
        require_accounts: $("net-require").checked,
      };
      if (preview) {
        msg.textContent = "Saved in this preview.";
        return;
      }
      try {
        await PJ.call("PUT", "/api/network", body);
        msg.textContent = "Saved.";
        renderNetwork();
      } catch (e) {
        msg.textContent = e.message;
      }
    });
    root.append(form);

    const people = el("div", null, "panel");
    people.append(el("h2", "People"));
    people.append(el("p", "These accounts can sign in and use this server. Revoking one stops new tokens and cuts off its devices.", "muted"));
    const table = document.createElement("table");
    const head = document.createElement("tr");
    ["Email", ""].forEach((h) => head.append(el("th", h)));
    table.append(head);
    const members = (data.members || []).filter((m) => !m.revoked_at);
    if (!members.length) {
      const tr = document.createElement("tr");
      const td = el("td", "No one else yet. You always have access.", "muted");
      td.colSpan = 2;
      tr.append(td);
      table.append(tr);
    }
    members.forEach((m) => {
      const tr = document.createElement("tr");
      tr.append(el("td", m.email));
      const td = document.createElement("td");
      const revoke = el("button", "Revoke", "ghost");
      revoke.type = "button";
      revoke.addEventListener("click", async () => {
        if (preview) {
          tr.remove();
          return;
        }
        await PJ.call("DELETE", "/api/network/members", { email: m.email });
        renderNetwork();
      });
      td.append(revoke);
      tr.append(td);
      table.append(tr);
    });
    people.append(table);
    const invite = el("form", null, "row");
    const email = document.createElement("input");
    email.type = "email";
    email.placeholder = "ada@example.com";
    email.required = true;
    const add = el("button", "Allow this account", "primary");
    add.type = "submit";
    invite.append(email, add);
    invite.addEventListener("submit", async (ev) => {
      ev.preventDefault();
      if (preview) {
        const tr = document.createElement("tr");
        tr.append(el("td", email.value), el("td", "Allowed"));
        table.append(tr);
        email.value = "";
        return;
      }
      await PJ.call("POST", "/api/network/members", { email: email.value });
      renderNetwork();
    });
    people.append(invite);
    root.append(people);

    const devices = el("div", null, "panel");
    devices.append(el("h2", "Devices"));
    devices.append(el("p", "A device that signed in, or enrolled with a one-time code. Revoke cuts it off.", "muted"));
    (data.devices || []).forEach((d) => {
      const row = el("div", null, "row");
      const label = `${d.name || d.device_id} · ${d.email}${d.revoked_at ? " · revoked" : ""}`;
      row.append(el("span", label));
      if (!d.revoked_at) {
        const btn = el("button", "Revoke", "ghost");
        btn.type = "button";
        btn.addEventListener("click", async () => {
          if (preview) {
            row.append(el("span", "revoked", "muted"));
            btn.remove();
            return;
          }
          await PJ.call("POST", "/api/network/revoke-device", { device_id: d.device_id });
          renderNetwork();
        });
        row.append(btn);
      }
      devices.append(row);
    });
    const codeBtn = el("button", "Create a one-time enrollment code", "ghost");
    codeBtn.type = "button";
    const codeOut = el("p", "", "muted");
    codeBtn.addEventListener("click", async () => {
      if (preview) {
        codeOut.textContent = "Code ABCD-EFGH, valid for one hour, one device.";
        return;
      }
      const got = await PJ.call("POST", "/api/network/enroll-code");
      codeOut.textContent = `Code ${got.code}. It works once, for one hour.`;
    });
    devices.append(codeBtn, codeOut);
    root.append(devices);

    const keys = el("div", null, "panel");
    keys.append(el("h2", "Network keys"));
    keys.append(el("p", "A copy-paste key joins this network without typing the server address. The key itself is shown once and is not stored here.", "muted"));
    const invites = data.invites || [];
    if (!invites.length) keys.append(el("p", "No network keys yet.", "muted"));
    invites.forEach((inv) => {
      const row = el("div", null, "key-row");
      const limit = inv.max_uses == null ? "no use limit" : `${inv.uses || 0} of ${inv.max_uses} uses`;
      const exp = inv.expires_at ? `expires ${date(inv.expires_at)}` : "no expiry";
      const bound = inv.bound_email ? `only ${inv.bound_email}` : "any device";
      const state = inv.revoked_at ? "revoked" : inv.mode === "embedded" ? "self-contained" : "passphrase";
      row.append(el("div", `${state} · ${limit} · ${exp} · ${bound}`));
      (inv.history || []).forEach((use) => {
        row.append(el("div", `${use.device_name || use.device_id} · ${use.email || "enrolled"} · ${date(use.used_at)}`, "muted"));
      });
      if (!inv.revoked_at) {
        const revoke = el("button", "Revoke", "ghost");
        revoke.type = "button";
        revoke.addEventListener("click", async () => {
          if (preview) {
            row.append(el("span", "revoked", "muted"));
            revoke.remove();
            return;
          }
          await PJ.call("POST", "/api/network/invites/revoke", { id: inv.id });
          renderNetwork();
        });
        row.append(revoke);
      }
      keys.append(row);
    });
    const make = el("button", "Create a network key", "primary");
    make.type = "button";
    make.addEventListener("click", () => openInvite(net));
    keys.append(make);
    root.append(keys);

    const comps = el("div", null, "panel");
    comps.append(el("h2", "Saved computers"));
    comps.append(el("p", "Names and IDs from this account. Passwords are not stored here.", "muted"));
    const list = data.computers || [];
    if (!list.length) comps.append(el("p", "No saved computers yet.", "muted"));
    list.forEach((c) => {
      const host = c.hostname || "";
      const nick = c.name && host && c.name.toLowerCase() !== host.toLowerCase() ? `${c.name} · ${host}` : c.name || host || "Computer";
      comps.append(el("div", `${nick} · ${c.id} · ${c.os_family || "unknown"} ${c.os_version || ""}`.trim()));
    });
    root.append(comps);
    if (preview && params.get("invite") === "1" && window.PJNetKey) {
      const secretHex = "11".repeat(32);
      PJNetKey.sealEmbedded({
        networkId: net.id || "net_office",
        secretHex,
        active: net.active_address,
        activeFingerprint: net.active_fingerprint,
        standby: net.standby_address || "",
        standbyFingerprint: net.standby_fingerprint || "",
      }).then((blob) => showInviteResult(blob, true));
    }
  }

  let inviteNet = null;

  function clearInviteSecret() {
    const blob = $("invite-blob");
    if (blob) blob.value = "";
    const img = $("invite-qr");
    if (img) img.removeAttribute("src");
    const pass = $("invite-pass");
    if (pass) pass.value = "";
  }

  function syncInviteMode() {
    const embedded = $("invite-mode").value === "embedded";
    $("invite-warn").classList.toggle("hidden", !embedded);
    $("invite-pass-label").classList.toggle("hidden", embedded);
  }

  function showInviteResult(blob, embedded) {
    $("invite-editor").classList.add("hidden");
    $("invite-result").classList.remove("hidden");
    $("invite-result-warn").classList.toggle("hidden", !embedded);
    $("invite-blob").value = blob;
    if (window.PJNetKey) {
      PJNetKey.qrDataUrl(blob).then((url) => {
        $("invite-qr").src = url;
      });
    }
    $("invite").classList.remove("hidden");
  }

  function openInvite(net) {
    inviteNet = net;
    clearInviteSecret();
    $("invite-editor").classList.remove("hidden");
    $("invite-result").classList.add("hidden");
    $("invite-msg").textContent = "";
    $("invite-mode").value = "passphrase";
    syncInviteMode();
    $("invite").classList.remove("hidden");
  }

  function closeInvite() {
    clearInviteSecret();
    $("invite").classList.add("hidden");
  }

  $("invite-mode").addEventListener("change", syncInviteMode);
  $("invite-pass-toggle").addEventListener("click", () => {
    const input = $("invite-pass");
    const show = input.type === "password";
    input.type = show ? "text" : "password";
    $("invite-pass-toggle").textContent = show ? "Hide" : "Show";
  });
  $("invite-cancel").addEventListener("click", closeInvite);
  $("invite-done").addEventListener("click", closeInvite);
  $("invite-copy").addEventListener("click", async () => {
    const text = $("invite-blob").value;
    try {
      await navigator.clipboard.writeText(text);
    } catch {
      $("invite-blob").focus();
      $("invite-blob").select();
    }
  });
  $("invite-form").addEventListener("submit", async (ev) => {
    ev.preventDefault();
    const net = inviteNet;
    const msg = $("invite-msg");
    if (!net || !net.active_address || !net.active_fingerprint) {
      msg.textContent = "Save the server address and its fingerprint first.";
      return;
    }
    if (!window.PJNetKey) {
      msg.textContent = "The network key tools did not load.";
      return;
    }
    const embedded = $("invite-mode").value === "embedded";
    const passphrase = $("invite-pass").value;
    if (!embedded && passphrase.length < 8) {
      msg.textContent = "Use a passphrase of at least 8 characters.";
      return;
    }
    const days = $("invite-days").value.trim();
    const maxText = $("invite-max").value.trim();
    let expiresAt = null;
    if (days) {
      const n = Number(days);
      if (!Number.isInteger(n) || n < 1) {
        msg.textContent = "Expiry has to be a number of days, or empty.";
        return;
      }
      expiresAt = Math.floor(Date.now() / 1000) + n * 86400;
    }
    let maxUses = null;
    if (maxText) {
      maxUses = Number(maxText);
      if (!Number.isInteger(maxUses) || maxUses < 1) {
        msg.textContent = "Maximum uses has to be at least 1, or leave it empty.";
        return;
      }
    }
    const secret = PJNetKey.randomBytes(32);
    const secretHex = [...secret].map((b) => b.toString(16).padStart(2, "0")).join("");
    const contents = {
      networkId: net.id || "net",
      secretHex,
      active: net.active_address,
      activeFingerprint: net.active_fingerprint,
      standby: net.standby_address || "",
      standbyFingerprint: net.standby_fingerprint || "",
    };
    try {
      if (!preview) {
        const hash = await PJNetKey.secretHash(secretHex);
        await PJ.call("POST", "/api/network/invites", {
          secret_hash: hash,
          mode: embedded ? "embedded" : "passphrase",
          bound_email: $("invite-bound").value.trim(),
          max_uses: maxUses,
          expires_at: expiresAt,
        });
      }
      const blob = embedded ? await PJNetKey.sealEmbedded(contents) : await PJNetKey.sealPassphrase(contents, passphrase);
      secret.fill(0);
      $("invite-pass").value = "";
      showInviteResult(blob, embedded);
      if (!preview) renderNetwork();
    } catch (e) {
      msg.textContent = e.message || "Could not create the network key.";
    }
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
