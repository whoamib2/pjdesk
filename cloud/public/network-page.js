// My network for the signed-in account. The Account page edits the network
// that account owns. The Admin page can show the same data read-only.
// Site admin is not required, and this script never calls /api/admin.
(() => {
  const PREVIEW = {
    role: "owner",
    note: "Computers that have not signed in are listed in the PJDesk Server window until you turn on Require signed-in accounts.",
    network: {
      id: "net_office",
      name: "Office",
      active_address: "office.example:21200",
      active_fingerprint: "ab12cd34ef56ab78",
      standby_address: "cabin.example:21200",
      standby_fingerprint: "0011223344556677",
      require_accounts: false,
    },
    members: [{ email: "ada@example.com", created_at: 1720000000, revoked_at: null }],
    devices: [
      { device_id: "office-pc", email: "ben@spader.us", name: "Office PC", enrolled_at: 1728000000, revoked_at: null },
      { device_id: "old-phone", email: "ada@example.com", name: "Old phone", enrolled_at: 1727000000, revoked_at: 1728500000 },
    ],
    computers: [
      { id: "123456789", name: "Front desk", hostname: "OFFICE-PC", os_family: "windows", os_version: "11" },
      { id: "555000111", name: "", hostname: "Pixel-8", os_family: "android", os_version: "15" },
    ],
    changes: [{ at: 1728200000, actor: "ben@spader.us", action: "register-active", detail: "office.example:21200" }],
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
  };

  let generation = 0;
  let refresh = async () => {};
  let inviteNet = null;

  function el(tag, text, cls) {
    const n = document.createElement(tag);
    if (text != null) n.textContent = text;
    if (cls) n.className = cls;
    return n;
  }

  function date(unix) {
    if (!unix) return "—";
    if (window.PJ && PJ.date) return PJ.date(unix);
    return new Date(unix * 1000).toLocaleString();
  }

  function field(label, id, value) {
    const wrap = el("label", label);
    const input = document.createElement("input");
    input.type = "text";
    input.id = id;
    input.value = value || "";
    input.autocomplete = "off";
    wrap.append(input);
    return wrap;
  }

  function ensureModal() {
    if (document.getElementById("nk-modal")) return;
    const modal = el("div", null, "net-modal hidden");
    modal.id = "nk-modal";
    modal.setAttribute("role", "dialog");
    modal.setAttribute("aria-modal", "true");
    modal.setAttribute("aria-labelledby", "nk-title");
    modal.innerHTML = `
      <form class="net-dialog" id="nk-form">
        <h2 id="nk-title">Network key</h2>
        <div id="nk-editor">
          <label>Protection
            <select id="nk-mode">
              <option value="passphrase">Passphrase, shared separately</option>
              <option value="embedded">Self-contained</option>
            </select>
          </label>
          <p id="nk-warn" class="net-msg err hidden">Anyone who has this network key can join. Share it like a password.</p>
          <label id="nk-pass-label">Passphrase
            <span class="net-row">
              <input id="nk-pass" type="password" autocomplete="new-password" minlength="8">
              <button type="button" id="nk-pass-toggle" class="btn btn-small">Show</button>
            </span>
          </label>
          <label>Expires in days (optional)
            <input id="nk-days" type="text" inputmode="numeric" placeholder="Leave empty for no expiry">
          </label>
          <label>Maximum uses (optional)
            <input id="nk-max" type="text" inputmode="numeric" placeholder="Leave empty for no limit">
          </label>
          <label>Limit to this account (optional)
            <input id="nk-bound" type="email" placeholder="ada@example.com">
          </label>
          <p id="nk-msg" class="net-msg err"></p>
          <div class="net-row">
            <button type="button" id="nk-cancel" class="btn">Cancel</button>
            <span style="flex:1"></span>
            <button type="submit" class="btn btn-primary">Create network key</button>
          </div>
        </div>
        <div id="nk-result" class="hidden">
          <p class="net-muted">Copy this now. It is shown once and is not saved here.</p>
          <p id="nk-result-warn" class="net-msg err hidden">Anyone who has this network key can join. Share it like a password.</p>
          <textarea id="nk-blob" readonly rows="4" spellcheck="false"></textarea>
          <img id="nk-qr" alt="QR code of the network key" width="180" height="180">
          <div class="net-row">
            <button type="button" id="nk-copy" class="btn btn-primary">Copy</button>
            <span style="flex:1"></span>
            <button type="button" id="nk-done" class="btn">Close</button>
          </div>
        </div>
      </form>`;
    document.body.append(modal);

    const $ = (id) => document.getElementById(id);
    function syncMode() {
      const embedded = $("nk-mode").value === "embedded";
      $("nk-warn").classList.toggle("hidden", !embedded);
      $("nk-pass-label").classList.toggle("hidden", embedded);
    }
    $("nk-mode").addEventListener("change", syncMode);
    $("nk-pass-toggle").addEventListener("click", () => {
      const input = $("nk-pass");
      const show = input.type === "password";
      input.type = show ? "text" : "password";
      $("nk-pass-toggle").textContent = show ? "Hide" : "Show";
    });
    function close() {
      $("nk-blob").value = "";
      $("nk-qr").removeAttribute("src");
      $("nk-pass").value = "";
      modal.classList.add("hidden");
    }
    $("nk-cancel").addEventListener("click", close);
    $("nk-done").addEventListener("click", close);
    $("nk-copy").addEventListener("click", async () => {
      const text = $("nk-blob").value;
      try {
        await navigator.clipboard.writeText(text);
      } catch {
        $("nk-blob").focus();
        $("nk-blob").select();
      }
    });
    $("nk-form").addEventListener("submit", async (ev) => {
      ev.preventDefault();
      const net = inviteNet;
      const msg = $("nk-msg");
      if (!net || !net.active_address || !net.active_fingerprint) {
        msg.textContent = "Save the server address and its fingerprint first.";
        return;
      }
      if (!window.PJNetKey) {
        msg.textContent = "The network key tools did not load.";
        return;
      }
      const embedded = $("nk-mode").value === "embedded";
      const passphrase = $("nk-pass").value;
      if (!embedded && passphrase.length < 8) {
        msg.textContent = "Use a passphrase of at least 8 characters.";
        return;
      }
      const days = $("nk-days").value.trim();
      const maxText = $("nk-max").value.trim();
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
      const preview = modal.dataset.preview === "1";
      try {
        if (!preview) {
          const hash = await PJNetKey.secretHash(secretHex);
          await PJ.call("POST", "/api/network/invites", {
            secret_hash: hash,
            mode: embedded ? "embedded" : "passphrase",
            bound_email: $("nk-bound").value.trim(),
            max_uses: maxUses,
            expires_at: expiresAt,
          });
        }
        const blob = embedded ? await PJNetKey.sealEmbedded(contents) : await PJNetKey.sealPassphrase(contents, passphrase);
        secret.fill(0);
        $("nk-pass").value = "";
        $("nk-editor").classList.add("hidden");
        $("nk-result").classList.remove("hidden");
        $("nk-result-warn").classList.toggle("hidden", !embedded);
        $("nk-blob").value = blob;
        PJNetKey.qrDataUrl(blob).then((url) => {
          $("nk-qr").src = url;
        });
        if (!preview) refresh();
      } catch (e) {
        msg.textContent = e.message || "Could not create the network key.";
      }
    });
  }

  function openInvite(net, preview) {
    ensureModal();
    inviteNet = net;
    const $ = (id) => document.getElementById(id);
    const modal = $("nk-modal");
    modal.dataset.preview = preview ? "1" : "0";
    $("nk-blob").value = "";
    $("nk-qr").removeAttribute("src");
    $("nk-pass").value = "";
    $("nk-editor").classList.remove("hidden");
    $("nk-result").classList.add("hidden");
    $("nk-msg").textContent = "";
    $("nk-mode").value = "passphrase";
    $("nk-warn").classList.add("hidden");
    $("nk-pass-label").classList.remove("hidden");
    modal.classList.remove("hidden");
  }

  function draw(root, data, opts) {
    const preview = !!opts.preview;
    const member = data.role === "member";
    const readonly = !!opts.readonly || member;
    const net = (data && data.network) || {
      name: "",
      active_address: "",
      active_fingerprint: "",
      standby_address: "",
      standby_fingerprint: "",
      require_accounts: false,
    };
    root.replaceChildren();
    if (opts.title) {
      root.append(el("h2", "My network", "net-title"));
      root.append(
        el(
          "p",
          "Servers, network keys, and who can connect. Any signed-in account manages its own network here.",
          "net-lead",
        ),
      );
    }
    if (opts.readonly) {
      const note = el("p", null, "net-lead");
      note.append(document.createTextNode("Read-only. Manage this network on your "));
      const link = el("a", "Account");
      link.href = "/account/";
      note.append(link, document.createTextNode(" page."));
      root.append(note);
    } else if (member) {
      root.append(el("p", "You use this network. The owner manages servers, keys, and who can join.", "net-lead"));
    }
    if (!data.network && readonly) {
      root.append(el("p", "This account has no network yet.", "net-lead"));
      return;
    }

    const form = el(readonly ? "div" : "form", null, "net-card");
    form.append(el("h2", "Server slots"));
    if (!readonly) {
      form.append(
        el(
          "p",
          "Signing in to PJDesk Server on the server computer fills the active or standby address and its key. You can still edit them here.",
          "net-muted",
        ),
      );
      form.append(field("Name", "net-name", net.name));
      form.append(field("Active server (host or host:port)", "net-active", net.active_address));
      form.append(field("Active server key fingerprint", "net-active-fp", net.active_fingerprint));
      form.append(field("Standby server", "net-standby", net.standby_address));
      form.append(field("Standby server key fingerprint", "net-standby-fp", net.standby_fingerprint));
      const requireRow = el("div", null, "net-check");
      const sw = el("label", null, "net-switch");
      const box = document.createElement("input");
      box.type = "checkbox";
      box.id = "net-require";
      box.checked = !!net.require_accounts;
      sw.append(box, document.createElement("span"));
      requireRow.append(sw, document.createTextNode("Require signed-in accounts"));
      form.append(requireRow);
      form.append(
        el(
          "p",
          data.note || "Turn this on only after every computer has signed in. Until then, computers that already registered keep working.",
          "net-muted",
        ),
      );
      const save = el("button", "Save network", "btn btn-primary");
      save.type = "submit";
      form.append(save);
      const msg = el("p", "", "net-msg");
      form.append(msg);
      form.addEventListener("submit", async (ev) => {
        ev.preventDefault();
        const body = {
          name: document.getElementById("net-name").value,
          active_address: document.getElementById("net-active").value,
          active_fingerprint: document.getElementById("net-active-fp").value,
          standby_address: document.getElementById("net-standby").value,
          standby_fingerprint: document.getElementById("net-standby-fp").value,
          require_accounts: document.getElementById("net-require").checked,
        };
        if (preview) {
          msg.className = "net-msg ok";
          msg.textContent = "Saved in this preview.";
          return;
        }
        try {
          await PJ.call("PUT", "/api/network", body);
          msg.className = "net-msg ok";
          msg.textContent = "Saved.";
          refresh();
        } catch (e) {
          msg.className = "net-msg err";
          msg.textContent = e.message;
        }
      });
    } else {
      const kv = el("dl", null, "net-kv");
      const rows = [
        ["Name", net.name || "—"],
        ["Active server", net.active_address || "—"],
        ["Active key", net.active_fingerprint || "—"],
        ["Standby server", net.standby_address || "—"],
        ["Standby key", net.standby_fingerprint || "—"],
        ["Require signed-in accounts", net.require_accounts ? "On" : "Off"],
      ];
      for (const [k, v] of rows) {
        kv.append(el("dt", k), el("dd", v, k.includes("key") ? "net-mono" : ""));
      }
      form.append(kv);
      if (data.note) form.append(el("p", data.note, "net-muted"));
    }
    root.append(form);

    if (readonly && member) {
      computers(root, data.computers || []);
      return;
    }

    const changes = data.changes || [];
    if (changes.length) {
      const log = el("div", null, "net-card");
      log.append(el("h2", "Changes"));
      log.append(el("p", "Who set or cleared a server. A different key is not replaced unless you confirm it.", "net-muted"));
      for (const change of changes) {
        const when = change.at ? date(change.at) : "";
        const what = {
          "register-active": "set the active server",
          "register-standby": "set the standby server",
          "clear-active": "cleared the active server",
          "clear-standby": "cleared the standby server",
          edit: "edited the network",
        }[change.action] || change.action;
        const detail = change.detail ? ` (${change.detail})` : "";
        log.append(el("p", `${when} — ${change.actor} ${what}${detail}`));
      }
      root.append(log);
    }

    const people = el("div", null, "net-card");
    people.append(el("h2", "Allowed accounts"));
    people.append(el("p", "These accounts can sign in and use this server. Revoking one stops new tokens and cuts off its devices.", "net-muted"));
    const table = document.createElement("table");
    const head = document.createElement("tr");
    head.append(el("th", "Email"));
    if (!readonly) head.append(el("th", ""));
    table.append(head);
    const members = (data.members || []).filter((m) => !m.revoked_at);
    if (!members.length) {
      const tr = document.createElement("tr");
      const td = el("td", "No one else yet. You always have access.", "net-muted");
      td.colSpan = readonly ? 1 : 2;
      tr.append(td);
      table.append(tr);
    }
    members.forEach((m) => {
      const tr = document.createElement("tr");
      tr.append(el("td", m.email));
      if (!readonly) {
        const td = document.createElement("td");
        const revoke = el("button", "Revoke", "btn btn-small");
        revoke.type = "button";
        revoke.addEventListener("click", async () => {
          if (preview) {
            tr.remove();
            return;
          }
          try {
            await PJ.call("DELETE", "/api/network/members", { email: m.email });
            refresh();
          } catch (e) {
            people.append(el("p", e.message, "net-msg err"));
          }
        });
        td.append(revoke);
        tr.append(td);
      }
      table.append(tr);
    });
    people.append(table);
    if (!readonly) {
      const invite = el("form", null, "net-row net-invite");
      const email = document.createElement("input");
      email.type = "email";
      email.placeholder = "ada@example.com";
      email.required = true;
      email.autocomplete = "off";
      const add = el("button", "Allow this account", "btn btn-primary");
      add.type = "submit";
      const inviteMsg = el("p", "", "net-msg");
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
        try {
          await PJ.call("POST", "/api/network/members", { email: email.value });
          refresh();
        } catch (e) {
          inviteMsg.className = "net-msg err";
          inviteMsg.textContent = e.message;
        }
      });
      people.append(invite, inviteMsg);
    }
    root.append(people);

    const devices = el("div", null, "net-card");
    devices.append(el("h2", "Enrolled devices"));
    devices.append(el("p", "A device that signed in, or enrolled with a one-time code. Revoke cuts it off.", "net-muted"));
    const list = data.devices || [];
    if (!list.length) devices.append(el("p", "No enrolled devices yet.", "net-muted"));
    list.forEach((d) => {
      const row = el("div", null, "net-row");
      const label = `${d.name || d.device_id} · ${d.email}${d.revoked_at ? " · revoked" : ""}`;
      row.append(el("span", label));
      if (!d.revoked_at && !readonly) {
        const btn = el("button", "Revoke", "btn btn-small");
        btn.type = "button";
        btn.addEventListener("click", async () => {
          if (preview) {
            row.append(el("span", "revoked", "net-muted"));
            btn.remove();
            return;
          }
          try {
            await PJ.call("POST", "/api/network/revoke-device", { device_id: d.device_id });
            refresh();
          } catch (e) {
            devices.append(el("p", e.message, "net-msg err"));
          }
        });
        row.append(btn);
      }
      devices.append(row);
    });
    if (!readonly) {
      const codeBtn = el("button", "Create a one-time enrollment code", "btn");
      codeBtn.type = "button";
      const codeOut = el("p", "", "net-muted");
      codeBtn.addEventListener("click", async () => {
        if (preview) {
          codeOut.textContent = "Code ABCD-EFGH, valid for one hour, one device.";
          return;
        }
        try {
          const got = await PJ.call("POST", "/api/network/enroll-code");
          codeOut.textContent = `Code ${got.code}. It works once, for one hour.`;
        } catch (e) {
          codeOut.textContent = e.message;
        }
      });
      devices.append(codeBtn, codeOut);
    }
    root.append(devices);

    const keys = el("div", null, "net-card");
    keys.append(el("h2", "Network keys"));
    keys.append(el("p", "A copy-paste key joins this network without typing the server address. The key itself is shown once and is not stored here.", "net-muted"));
    const invites = data.invites || [];
    if (!invites.length) keys.append(el("p", "No network keys yet.", "net-muted"));
    invites.forEach((inv) => {
      const row = el("div", null, "net-key");
      const limit = inv.max_uses == null ? "no use limit" : `${inv.uses || 0} of ${inv.max_uses} uses`;
      const exp = inv.expires_at ? `expires ${date(inv.expires_at)}` : "no expiry";
      const bound = inv.bound_email ? `only ${inv.bound_email}` : "any device";
      const state = inv.revoked_at ? "revoked" : inv.mode === "embedded" ? "self-contained" : "passphrase";
      row.append(el("div", `${state} · ${limit} · ${exp} · ${bound}`));
      (inv.history || []).forEach((use) => {
        row.append(el("div", `${use.device_name || use.device_id} · ${use.email || "enrolled"} · ${date(use.used_at)}`, "net-muted"));
      });
      if (!inv.revoked_at && !readonly) {
        const revoke = el("button", "Revoke", "btn btn-small");
        revoke.type = "button";
        revoke.addEventListener("click", async () => {
          if (preview) {
            row.append(el("span", "revoked", "net-muted"));
            revoke.remove();
            return;
          }
          try {
            await PJ.call("POST", "/api/network/invites/revoke", { id: inv.id });
            refresh();
          } catch (e) {
            keys.append(el("p", e.message, "net-msg err"));
          }
        });
        row.append(revoke);
      }
      keys.append(row);
    });
    if (!readonly) {
      const make = el("button", "Create a network key", "btn btn-primary");
      make.type = "button";
      make.addEventListener("click", () => openInvite(net, preview));
      keys.append(make);
    }
    root.append(keys);
    computers(root, data.computers || []);
  }

  function computers(root, list) {
    const comps = el("div", null, "net-card");
    comps.append(el("h2", "Saved computers"));
    comps.append(el("p", "Names and IDs from this account. Passwords are not stored here.", "net-muted"));
    if (!list.length) comps.append(el("p", "No saved computers yet.", "net-muted"));
    list.forEach((c) => {
      const host = c.hostname || "";
      const nick = c.name && host && c.name.toLowerCase() !== host.toLowerCase() ? `${c.name} · ${host}` : c.name || host || "Computer";
      comps.append(el("div", `${nick} · ${c.id} · ${c.os_family || "unknown"} ${c.os_version || ""}`.trim()));
    });
    root.append(comps);
  }

  async function render(root, opts = {}) {
    const gen = ++generation;
    const preview = !!opts.preview;
    root.replaceChildren(el("p", "Loading…", "net-muted"));
    refresh = async () => render(root, opts);
    let data;
    try {
      data = preview ? PREVIEW : await PJ.call("GET", "/api/network");
    } catch (e) {
      if (gen !== generation) return;
      root.replaceChildren(el("p", e.message || "Could not load your network.", "net-msg err"));
      return;
    }
    if (gen !== generation) return;
    draw(root, data || { network: null }, opts);
  }

  window.PJNetwork = { render };
})();
