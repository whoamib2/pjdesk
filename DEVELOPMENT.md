# PJDesk — Development & Handoff

> **Audience:** the next engineer or AI picking up this project mid‑stream.
> **Read §1 before touching anything** — the server and the public site are
> live in production and a careless change (or a published release) updates
> real users automatically.

PJDesk is a self‑hosted, open‑source RustDesk‑style remote desktop app: a
**client** that both shares this screen (host) and controls another machine
(controller), and a **server** that does signaling, NAT rendezvous and relay.
Repo: `https://github.com/whoamib2/pjdesk` (public). Site: `https://pjdesk.com`.

---

## 1. 🚨 PRODUCTION & CRITICAL CONSTRAINTS

### What is LIVE right now
| Thing | Where | Notes |
|---|---|---|
| **PJDesk server** | the address you sign in to (TCP signaling + UDP rendezvous/relay, port 21200) | Windows service **`PJDeskServer`**, `C:\Program Files\PJDesk Server\pjdesk-server.exe`, data in `C:\ProgramData\PJDesk\server`. Real clients connect through it. |
| **Public download site** | `https://pjdesk.com` | GitHub Pages from the **`gh-pages`** branch (has `CNAME`). Porkbun DNS A‑records → GitHub Pages IPs. Currently serves the **0.2.0** installers. |
| **Auto‑update feed** | GitHub Releases of `whoamib2/pjdesk` | The updater (`pjdesk-proto::update`) reads `releases/latest` and **silently installs any newer version as SYSTEM**. |
| **Deployed client** | one machine (`benlenovodesk`), device ID `326 135 922` | Running "Build 1" (`pjdesk.exe` + two bundled MinGW DLLs — see §3). End‑to‑end connect to the live server is confirmed working. |

### Do‑Not‑Touch zones / architectural rules
1. **Wire protocol changes need a coordinated release.** `PROTOCOL_VERSION = 3`
   (2 is still accepted). A peer that only speaks 2 keeps the classic desktop
   session. New messages are appended, so an old peer never has to decode them.
   (`crates/pjdesk-proto/src/lib.rs`). New postcard variants are appended, and
   a version-2 peer is never sent a version-3 message. Do not add a field to
   an existing postcard struct: that still breaks a peer that speaks the older
   layout. `#[serde(default)]` is safe on the JSON files (accounts, devices,
   the cloud bundle), not on postcard messages.
2. **Never rotate the server TLS identity casually.** Clients trust the server
   key **on first use (TOFU)** and pin its SHA‑256 (`server_fingerprint` in the
   client config). Deleting/regenerating `server-cert.pem` / `server-key.pem`
   forces **every** client to re‑pin by hand (`pjdesk config --reset-fingerprint`).
   This already bit us once during troubleshooting.
3. **Server data‑folder permissions: only ever `icacls /reset`.** See
   `repair_data_dir` in `crates/pjdesk-server/src/service.rs`. **Do NOT** restore
   the old `icacls /inheritance:r /grant SID:(OI)(CI)F /T` approach — the
   container‑inherit flags `(OI)(CI)` are invalid on files, so icacls strips each
   file's inherited ACL and then fails to re‑grant, leaving an **empty DACL** that
   denies SYSTEM itself and crashes the server with *"Access is denied reading
   devices.json"*. That was a production outage; the current `/reset` approach is
   the fix.
4. **The server management window is read‑only w.r.t. the identity.** `gui.rs`
   `refresh()` must use `server::read_identity` (never `load_identity` /
   `repair_data_dir`) — otherwise it regenerates the key and flashes console
   windows on every refresh.
5. **Publishing a GitHub Release IS a production deploy.** A release tagged higher
   than installed (e.g. `v0.3.0`) auto‑installs on all clients within the update
   interval, silently, as SYSTEM, via NSIS `/S`. Never publish an untested
   installer. (Today there is **no** release newer than `0.2.0`, so nothing is
   auto‑updating — this is why `pjdesk.com` still shows 0.2.0.)
6. **The client host runs as the signed‑in user, not SYSTEM.** `spawn_session_host`
   uses `WTSQueryUserToken` (`crates/pjdesk/src/service.rs`). Reverting to a SYSTEM
   token re‑introduces the session‑host "flapping" and breaks DPAPI secret
   decryption (secrets are user‑scoped). Do not change this.
7. **Permanent password is never displayed.** `gui/panels.rs` shows it masked with
   no reveal/copy by design. Don't add a reveal path back.
8. **Keep `pjdesk.com` infra intact:** the `gh-pages` branch's `CNAME` file and the
   Porkbun A‑records. Losing either takes the site down.

---

## 2. 🛠️ TECH STACK & ARCHITECTURE MAP

### Language / build
- **Rust** (edition 2021, `rust-version = 1.85`), Cargo **workspace**, 3 crates.
- Ships as **Windows** binaries, cross‑compiled from Linux to
  **`x86_64-pc-windows-gnu`** (MinGW, win32 thread model). MSVC targets also build
  (`.cargo/config.toml` sets `+crt-static` for `*-pc-windows-msvc`).
- Release profile: `opt-level=3`, `lto="thin"`, `codegen-units=4`, `strip=true`.
- Installers: **NSIS** (`makensis`), silent install via `/S`.

### Key dependencies (versions pinned in `Cargo.toml`)
| Area | Crate(s) |
|---|---|
| Async runtime | `tokio` 1 |
| Transport | `quinn` 0.11 (QUIC, tokio + rustls‑ring) |
| TLS | `rustls` 0.23 (ring, TLS 1.3), `tokio-rustls` 0.26, `rcgen` 0.14 (self‑signed certs) |
| Auth | `spake2` 0.4 (PAKE password auth, bound to the TLS session) |
| Wire format | `postcard` 1, `serde` / `serde_json` |
| Hashing | `sha2`, `hmac`, `hex` |
| Video | **`openh264` 0.9** (Cisco OpenH264, C++ compiled via the `cc` crate; NASM for SIMD) |
| Viewer window | `winit` 0.30 + `softbuffer` 0.4 (CPU framebuffer); `ab_glyph` 0.2 + `epaint_default_fonts` 0.36 for the custom toolbar text |
| App GUIs | `eframe`/`egui` 0.36 (features: `default_fonts`, `glow`, `wgpu`) |
| Clipboard | `arboard` 3 |
| Windows APIs | `windows` 0.62, `windows-service`, `rpassword` 7 |

### Constants (`crates/pjdesk-proto/src/lib.rs`)
- `UPDATE_REPO = "whoamib2/pjdesk"`
- `PROTOCOL_VERSION = 3` (`MIN_PROTOCOL_VERSION = 2`)
- `DEFAULT_PORT = 21200`
- `ALPN_SIGNAL = b"pjdesk-signal/1"`, `ALPN_PEER = b"pjdesk-peer/1"`

### Directory / file map
```
pjdesk/
├── Cargo.toml                         # workspace: version 0.2.0, shared deps, profiles
├── .cargo/config.toml                 # +crt-static for MSVC targets (gnu is plain — see §3)
├── README.md  LICENSE
├── DEVELOPMENT.md                     # this file
│
├── crates/pjdesk-proto/src/           # shared wire/crypto library (no I/O policy)
│   ├── lib.rs                         # constants (UPDATE_REPO, PROTOCOL_VERSION, DEFAULT_PORT, ALPN)
│   ├── tls.rs                         # TlsIdentity (load/create/load_or_create), fingerprint PINNING (TOFU)
│   ├── auth.rs                        # SPAKE2 password handshake
│   ├── signal.rs                      # signaling messages (ClientMsg/ServerMsg), AddressEntry
│   ├── peer.rs                        # ControlMsg/HostMsg/InputEvent/SpecialCommand + H.264 frame framing
│   ├── framing.rs  udp.rs             # length-prefixed framing; UDP rendezvous/punch
│   └── update.rs                      # self-update: GitHub releases/latest → verify SHA256SUMS → run /S
│
├── crates/pjdesk-server/src/          # the server (bin: pjdesk-server)
│   ├── main.rs                        # CLI (run | service | account | update | fingerprint)
│   ├── server.rs                      # run(): TCP signaling + UDP rendezvous/relay; load_identity / read_identity; calls repair_data_dir
│   ├── service.rs                     # Windows service "PJDeskServer" + repair_data_dir (icacls /reset)
│   ├── gui.rs                         # management window (eframe): PJDesk account sign-in, service, fingerprint, devices, log
│   ├── accounts.rs                    # older local accounts (optional). The server sign-in is the PJDesk account
│   └── registry.rs                    # devices.json; Registry::load self-heals (empty on unreadable/corrupt)
│
├── crates/pjdesk/src/                 # the client (bin: pjdesk) — host + controller + GUI
│   ├── main.rs                        # CLI (default=GUI; host|connect|config|login|service|__session-host); ensure_console()
│   ├── config.rs                      # client.json: server, server_fingerprint, device_id/secret, Settings, Account, saved passwords
│   ├── secure.rs                      # DPAPI encrypt/decrypt (user-scoped) for secrets
│   ├── service.rs                     # Windows service "PJDesk"; session-host via WTSQueryUserToken; UPDATE_TASK "PJDesk Update" (schtasks)
│   ├── platform.rs  syspaths.rs       # elevation (run_elevated), paths, service status file
│   ├── p2p.rs  signaling.rs  session.rs  account.rs   # connection setup, signaling client, session auth, account login
│   ├── gui/
│   │   ├── mod.rs                      # App (eframe); connect_to() spawns the viewer (CREATE_NO_WINDOW)
│   │   ├── panels.rs                  # share/control panels; permanent password masked (no reveal)
│   │   ├── settings.rs                # Settings window (server addr, security, display, updates)
│   │   ├── theme.rs  widgets.rs
│   ├── host/mod.rs                    # screen capture + H.264 encode + input injection + monitor(s)
│   └── viewer/
│       ├── mod.rs                     # controller: connect()
│       ├── window.rs                  # winit+softbuffer viewer; zoom/pan state; send-keys; input forwarding
│       ├── render.rs                  # draw(): fit / 1:1 / arbitrary scale with crop+pan (bilinear)
│       ├── toolbar.rs                 # custom-drawn toolbar with Zoom + Send-keys drop-down menus
│       ├── keymap.rs  decoder.rs      # scancode map; H.264 decode
│
└── installer/
    ├── build.sh                       # builds both NSIS installers: installer/build.sh <version>
    ├── pjdesk-client.nsi              # → PJDesk-Client-Setup-<ver>.exe (also copied to PJDesk-Setup-<ver>.exe)
    └── pjdesk-server.nsi              # → PJDesk-Server-Setup-<ver>.exe (installs to C:\Program Files\PJDesk Server)
```

### Runtime layout (Windows)
| | Client (`pjdesk`) | Server (`pjdesk-server`) |
|---|---|---|
| Install dir | `C:\Program Files\PJDesk` | `C:\Program Files\PJDesk Server` |
| Service name | `PJDesk` | `PJDeskServer` |
| Data dir | `C:\ProgramData\PJDesk\service` (service `client.json`) | `C:\ProgramData\PJDesk\server` |
| GUI config | `%APPDATA%\PJDesk\client.json` | — |
| Data files | `client.json` | `server-cert.pem`, `server-key.pem`, `devices.json`, `accounts.json`, `server.log` |
| Firewall rule | `PJDesk` | `PJDesk Server` (TCP+UDP 21200) |
| Update task | schtasks `PJDesk Update` (SYSTEM) | via installer / `pjdesk-server update` |

---

## 3. 📍 CURRENT STATE & HANDOFF LOG

### ✅ Completed, stable, deployed
- **Server**: signaling + rendezvous + relay; management GUI (service control,
  **stable** key fingerprint, accounts, registered computers, log tail).
- **Server reliability fixes** (shipped as `pjdesk-server-fixed3.exe`):
  `repair_data_dir` (icacls /reset) + `Registry::load` self‑heal → fixes the
  "Access is denied / service won't start" outage; GUI no longer flashes or
  rotates the key.
- **Client "Build 1"** (committed `ada0e70`; deployed as `pjdesk-b1.exe`):
  - No console window pops up on connect (`ensure_console` attaches only; viewer
    spawned `CREATE_NO_WINDOW`).
  - Viewer **zoom**: Fit / 100% (1:1) / 50–400% via a toolbar menu, **Ctrl+scroll**
    zoom, center‑preserving, follow‑cursor pan when zoomed in (`render.rs`,
    `window.rs`).
  - **Send‑keys** menu: Ctrl+Alt+Del (secure path), Windows key, Alt+Tab, Ctrl+Esc,
    Win+D/E/L, Task Manager, Alt+F4, Print Screen (`toolbar.rs`, `window.rs`).
  - **Permanent password never displayed** (masked, no reveal/copy) (`panels.rs`).
- **End‑to‑end**: controlling the live server works; client registered and online.
- **pjdesk.com** live (0.2.0 installers, GitHub Pages + Porkbun DNS).

### 🔧 In progress — EXACT resume point
**Current focus: the UPDATE PIPELINE** (so remote machines update themselves).

**Immediate blocker — single‑file static build:** the gnu cross‑build links
**`libstdc++-6.dll`** dynamically, pulled in by OpenH264's C++ (the `cc` crate
auto‑links `stdc++` as a *dynamic* lib, and Rust marks it `-Wl,-Bdynamic`, which
overrides a global `-static`). Attempts that did **not** fix it: `+crt-static`,
`+crt-static` + `-static`, and the plain default build (all still import
`libstdc++-6.dll`).

- **State of `.cargo/config.toml`**: the experimental `[target.x86_64-pc-windows-gnu]`
  rustflags block was just **reverted** (gnu is back to plain). This commit is in
  the handoff.
- **Next action (do this first):** stop `cc` from linking stdc++ dynamically and
  link the **static** archive instead, then verify. Concretely:
  - Build with `CXXSTDLIB=""` (empty → the `cc` crate links no C++ stdlib), **and**
    add a static libstdc++ link via rustflags (`-l static=stdc++`, or
    `-C link-arg=-l:libstdc++.a`). The static archive exists at
    `/usr/lib/gcc/x86_64-w64-mingw32/13-win32/libstdc++.a`.
  - **Verify:** `x86_64-w64-mingw32-objdump -p target/x86_64-pc-windows-gnu/release/pjdesk.exe | grep -i "DLL Name"` must show **no** `libstdc++`, `libgcc`, or `libwinpthread`.
- **Interim production workaround (currently deployed):** `pjdesk-b1.exe` ships
  with **`libstdc++-6.dll`** and **`libgcc_s_seh-1.dll`** copied next to it in
  `C:\Program Files\PJDesk`. Remove these once the static build lands (and make
  sure `installer/pjdesk-client.nsi` does **not** install stale DLLs).

**Not started yet (rest of the pipeline):** version bump → build installers →
publish release → `pjdesk.com` refresh → 30‑minute auto‑update → server‑side
force‑update + per‑client version display.

### 🐞 Known bugs / tech debt / temporary workarounds (live in code)
- **Bundled MinGW DLLs** beside the client exe (above) — temporary until the
  static build.
- **Duplicate client registrations:** one physical machine appears twice in the
  server because the **GUI config** (`%APPDATA%\PJDesk\client.json`) and the
  **service config** (`C:\ProgramData\PJDesk\service\client.json`) each hold their
  own `device_id`. The client's ID has also changed across sessions
  (533783058 → 326135922) after config resets. Needs: unify the identity (share
  one `device_id`/secret) and/or dedupe on re‑registration, then purge existing
  duplicates from `devices.json`.
- **No version bump:** every fix so far ships as **0.2.0**, so `pjdesk.com` and the
  in‑app version are stale and auto‑update can't trigger. Fixed by the pipeline.
- **No code signing:** Windows SmartScreen shows *"Unknown publisher"* on install
  (owner chose to defer signing — needs a paid cert).
- **`repair_data_dir` grants `Users` read** of `server-key.pem` (inherited ACL via
  `/reset`). Acceptable on a single‑owner box; revisit if multi‑user.
- **Session‑host flapping:** the client service log showed `session-host` respawning
  ~1×/s last night. Most likely a symptom of the server being down at the time;
  **re‑verify** now that the server is healthy before treating it as a live bug.

---

## 4. 🏁 IMMEDIATE NEXT STEPS (do in order)

1. **Make the client a single self‑contained `.exe`.** Build with `CXXSTDLIB=""`
   + a static libstdc++ link (see §3); `objdump` must show no `libstdc++`/`libgcc`/
   `libwinpthread`. Rebuild the server too (it has no C++ dep but needs the new
   version). Bake the working flags into `.cargo/config.toml` (and/or `build.sh`)
   so releases are reproducible.
2. **Bump the version** `0.2.0 → 0.3.0` in `Cargo.toml` `[workspace.package]`.
3. **Build the installers:** `cargo build --release --target x86_64-pc-windows-gnu`
   then `installer/build.sh 0.3.0` → `PJDesk-Setup-0.3.0.exe`,
   `PJDesk-Server-Setup-0.3.0.exe`, `SHA256SUMS`.
4. **Publish the release (production push — test on ONE client first):**
   `gh release create v0.3.0 installer/PJDesk-Setup-0.3.0.exe installer/PJDesk-Server-Setup-0.3.0.exe installer/SHA256SUMS -t "PJDesk 0.3.0" -n "<notes>"`,
   then update the `pjdesk.com` (`gh-pages`) download page to 0.3.0.
5. **Switch auto‑update to every 30 min:** in `crates/pjdesk/src/service.rs`
   `set_update_task`, change the schtasks schedule from `/SC DAILY /ST 03:00` to
   `/SC MINUTE /MO 30`; mirror for the server and in the `.nsi` installers. Confirm
   a client actually pulls 0.3.0 from the release.

*(After the pipeline: server‑side **force‑update** + **per‑client version** display
in the server window; then Build 2 = clipboard sync + multi‑monitor tabs; then UI
/ installer polish; then optional code signing.)*

---

## 5. 🔐 ENV & INFRASTRUCTURE REQUIREMENTS

### Build toolchain (Linux cross‑build)
- Rust stable + `rustup target add x86_64-pc-windows-gnu`.
- **MinGW**: `x86_64-w64-mingw32-gcc` / `-g++` (win32 thread model).
- **NASM** (`nasm`) — OpenH264 SIMD (optional but expected).
- **NSIS** (`makensis`) — to build installers.
- Build: `cargo build --release --target x86_64-pc-windows-gnu` then
  `installer/build.sh <version>`. (There is **no** CI workflow yet — builds/releases
  are manual.)

### Secrets / credentials
- **No `.env`, API keys, or webhooks** are needed to *run* the apps.
- **At rest:** client secrets (`permanent_password`, account `token`, saved
  connection passwords) are **DPAPI‑encrypted, user‑scoped** via `crate::secure`.
  This is why the host must run as the signed‑in user (§1.6).
- **To publish:** a GitHub token / `gh auth login` with push + release rights on
  `whoamib2/pjdesk` (releases **and** the `gh-pages` branch).

### Networking
- Server listens on **TCP + UDP 21200** (`DEFAULT_PORT`). The name you set on
  pjdesk.com → Account → My network must resolve to the server host and have **both** TCP
  and UDP 21200 port‑forwarded/allowed (firewall rule `PJDesk Server`).
- Clients reach GitHub directly over HTTPS for updates
  (`https://api.github.com/repos/whoamib2/pjdesk/releases/latest` + asset
  downloads) — no server involvement.

### DNS / hosting (`pjdesk.com`)
- **GitHub Pages** served from the **`gh-pages`** orphan branch; that branch
  contains a **`CNAME`** file with `pjdesk.com`.
- **Porkbun** DNS: A‑records for the apex → GitHub Pages IPs
  `185.199.108.153`, `185.199.109.153`, `185.199.110.153`, `185.199.111.153`.
- The download page links the release installers; update it on every release.

### Release contract (what the auto‑updater requires)
A GitHub Release whose **tag** parses to a version **higher** than installed
(e.g. `v0.3.0`), containing assets named:
- `PJDesk-Client-Setup-<ver>.exe` (client installer, NSIS, supports `/S`)
- `PJDesk-Setup-<ver>.exe` (the same bytes; installed 0.5.x and 0.6.x clients look up this name)
- `PJDesk-Server-Setup-<ver>.exe` (server installer)
- `SHA256SUMS` (lines `"<sha256>  <filename>"`; the updater verifies the installer
  it downloads against this). Both client filenames are listed, with the same hash.
Miss the naming or the checksum and auto‑update silently no‑ops or fails.

---

*Last updated: 2026‑10‑16. Maintainer: Ben Spader (`whoamib2`). Keep §1 and §3
current — they are what stops the next person from breaking production.*
