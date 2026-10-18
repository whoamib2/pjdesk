(() => {
  const $ = (id) => document.getElementById(id);
  let accounts = [];

  function cell(text, cls) {
    const td = document.createElement("td");
    td.textContent = text;
    if (cls) td.className = cls;
    return td;
  }

  function render() {
    const q = $("filter").value.trim().toLowerCase();
    const shown = accounts.filter((a) => !q || a.email.includes(q));
    $("count").textContent = `${shown.length} of ${accounts.length}`;
    $("rows").replaceChildren(...shown.map((a) => {
      const tr = document.createElement("tr");
      const email = cell(a.email);
      if (a.is_admin) {
        const b = document.createElement("span");
        b.className = "pill on";
        b.style.marginLeft = "8px";
        b.textContent = "admin";
        email.appendChild(b);
      }
      tr.append(
        email,
        cell(PJ.date(a.created_at), "muted"),
        cell(PJ.date(a.last_login_at), "muted"),
        cell(String(a.signed_in || 0), "muted"),
        cell(a.synced_at ? `${PJ.date(a.synced_at)} · ${PJ.size(a.size)}` : "—", "muted"),
      );
      const td = document.createElement("td");
      const label = document.createElement("label");
      label.className = "switch";
      label.style.margin = "0";
      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = a.sync_enabled;
      box.disabled = a.is_admin;
      box.setAttribute("aria-label", `Cloud Sync for ${a.email}`);
      box.onchange = async () => {
        box.disabled = true;
        try {
          await PJ.call("POST", "/api/admin/sync", { email: a.email, enabled: box.checked });
          a.sync_enabled = box.checked;
          PJ.message($("msg"), `Cloud Sync ${box.checked ? "turned on" : "turned off"} for ${a.email}.`, "ok");
        } catch (e) {
          box.checked = !box.checked;
          PJ.message($("msg"), e.message, "err");
        } finally {
          box.disabled = a.is_admin;
        }
      };
      label.append(box, document.createElement("span"));
      td.appendChild(label);
      tr.appendChild(td);
      return tr;
    }));
  }

  async function load() {
    if (!PJ.token()) { location.href = "/account/"; return; }
    try {
      accounts = (await PJ.call("GET", "/api/admin/accounts")).accounts;
      render();
    } catch (e) {
      if (e.status === 401) { PJ.setToken(null); location.href = "/account/"; return; }
      PJ.message($("msg"), e.message, "err");
    }
  }

  $("filter").addEventListener("input", render);
  load();
})();
