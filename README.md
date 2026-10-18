# PJDesk

A self-hosted, open-source remote desktop in the spirit of RustDesk, written in Rust. Your computers connect only through a server you run — no cloud, no accounts with anyone but you. [pjdesk.com](https://pjdesk.com)

* **`pjdesk.exe`** — the app. A window to share this computer (ID + password) and control others, with a settings window, a sign-in to your server, a synced address book, and an optional background service so you can connect at the sign-in screen and during UAC prompts.
* **`pjdesk-server.exe`** — the hub. Signaling, NAT rendezvous and relay, with a management window for the service, user accounts and registered computers. Runs in a console or as a Windows service.

Connections are **peer-to-peer** whenever UDP hole punching works, and are **relayed** through your server when it doesn't. Video is **end-to-end encrypted** either way (QUIC/TLS 1.3). Passwords are checked with **SPAKE2**, so they never travel over the network, not even to your own server.

> Status: **v0.5.0.** See the [changelog](CHANGELOG.md). A 0.3.7 app still connects for a classic desktop session. Cloud Sync can force one server to be the primary (`pjdesk-server cloud promote`), and PJDesk Server can install its own updates when you turn that on.

---

## Getting started

1. **Run the server.** Install `PJDesk-Server-Setup` on a computer that stays on (or build it and run `pjdesk-server`). Tick "run as a service" so it starts with Windows. Note its address — a hostname or LAN IP — and, for use over the internet, forward TCP + UDP 21200 to it.
2. **Install the app.** Run `PJDesk-Setup` on each computer. Open PJDesk, go to **Settings ▸ Network**, and enter your server's address.
3. **Connect.** Your computer now shows a 9-digit **ID** and a password. Give those to someone, or type another computer's ID and password to control it.
4. **(Optional) Always-on access.** In **Settings ▸ Security**, install the **background service** to reach this computer before anyone signs in and during UAC prompts. Set a permanent password there too.
5. **(Optional) Sign in.** Create an account on the server (its window, or `pjdesk-server account add <name>`), then sign in from the app to sync an **address book** of saved computers across your machines.

---

## Architecture

```text
                         ┌─────────────────────────────────────────┐
                         │        pjdesk-server  (Windows)       │
                         │                                         │
                         │  TCP 21200  TLS 1.3 signaling           │
                         │    • device IDs (registry: devices.json)│
                         │    • session setup                      │
                         │  UDP 21200  rendezvous + relay          │
                         │    • learns each peer's public address  │
                         │    • forwards QUIC packets if P2P fails │
                         └──────▲──────────────────────────▲───────┘
                     signaling  │ (TLS)          (TLS)     │  signaling
                     + UDP bind │                          │  + UDP bind
         ┌──────────────────────┴───┐                  ┌───┴────────────────────────┐
         │  pjdesk.exe  HOST      │   QUIC (UDP)     │  pjdesk.exe  CONTROLLER   │
         │  DXGI/GDI capture        │◄════════════════►│  winit window + softbuffer  │
         │  BGRA→I420→H.264         │ direct P2P, or   │  H.264 decode → 0RGB        │
         │  SendInput (mouse/keys)  │ via server relay │  mouse/keyboard capture     │
         └──────────────────────────┘                  └─────────────────────────────┘
```

### How a connection is made

1. **Host registers.** On first run the host makes a random 32-byte device secret. It sends the secret over TLS, and the server gives it a permanent 9-digit **ID**, storing only the secret's SHA-256. The host shows its ID and a **one-time password**, which is generated locally and never sent anywhere.
2. **Controller asks for the ID.** The server tells the host, and the host accepts unless another controller is already connected (one at a time).
3. **UDP rendezvous.** Each side opens one UDP socket for this session and sends a `Bind` datagram (with a session token) to the server's UDP port. The server records each socket's **public address** (its NAT mapping) and both sides report their LAN addresses. The server then sends each peer the other's addresses, plus the host's certificate fingerprint for the controller.
4. **Hole punching.** Both sides fire small "punch" datagrams at each other's candidates, so each NAT expects the other's traffic. The controller then **races QUIC connections** to every candidate.
5. **Relay fallback.** If nothing answers within 3 s (for example a symmetric NAT or strict firewall), the controller connects to the **server's UDP port** instead. The server forwards every packet from one bound address to the other, like a minimal TURN server. QUIC is end-to-end encrypted, so the relay only sees ciphertext.
6. **Password check (SPAKE2).** The two sides run SPAKE2 with the password and each proves it got the same key by sending an HMAC over the TLS session's exporter (channel binding). A wrong password, or anyone in the middle (even a malicious server), fails without learning the password.
7. **Streaming.** The host opens a video stream and the controller sends input and frame acknowledgements on the control stream (details below).

### Low-latency streaming

* **Capture.** The host uses DXGI Desktop Duplication, where the GPU hands over the desktop only when it changes, so an idle screen costs nothing. If that isn't available (some VMs, RDP sessions, rotated screens) it falls back to GDI `BitBlt` with change detection. The mouse cursor is drawn into the image.
* **Encode.** OpenH264 in screen-content real-time mode, with no B-frames and no lookahead, so every frame comes out immediately. Colors use full-range BT.601, the same in both directions.
* **Flow control.** At most **3 frames are in flight**. The host only captures a new frame when the controller has decoded an old one, so frames never pile up in buffers. On a slow link you get fewer frames per second, not more delay.
* **Transport.** QUIC over UDP: no TCP head-of-line blocking between input and video, and input has priority over video. Keep-alives every 3 s keep NAT mappings open.
* **Decode and draw.** OpenH264 decodes into a CPU framebuffer (softbuffer), with a 1:1 copy or multi-threaded bilinear scaling.

On a LAN the expected glass-to-glass latency is about 30–60 ms at 1080p/30 fps. The viewer title bar shows live fps, bitrate, RTT and host encode time.

---

## Project structure

```text
PJDesk/
├── Cargo.toml                      workspace (shared dependency versions)
├── .cargo/config.toml              static C runtime on Windows (single-file .exe)
├── build.cmd                       one-step Windows build → .\dist
├── CHANGELOG.md                    what changed in each version
├── crates/
│   ├── pjdesk-proto/             wire protocol shared by server and client
│   │   └── src/
│   │       ├── signal.rs           signaling messages (+ sequence diagram)
│   │       ├── udp.rs              rendezvous datagrams (Bind / BindAck / Punch)
│   │       ├── peer.rs             auth, control, input and video frame formats
│   │       ├── framing.rs          length-prefixed postcard framing
│   │       ├── auth.rs             SPAKE2 + channel-bound key confirmation
│   │       └── tls.rs              self-signed identities, fingerprint pinning
│   ├── pjdesk-server/            the server
│   │   └── src/
│   │       ├── main.rs             CLI: run | fingerprint | service ...
│   │       ├── server.rs           TLS signaling, sessions, rate limiting
│   │       ├── relay.rs            UDP rendezvous + packet relay
│   │       ├── registry.rs         persistent ID registry (devices.json)
│   │       └── service.rs          Windows service install/uninstall/run
│   └── pjdesk/                   the client (host + controller)
│       └── src/
│           ├── main.rs             CLI + double-click interactive mode
│           ├── config.rs           %APPDATA%\PJDesk\client.json
│           ├── signaling.rs        TLS connection to the server (TOFU pinning)
│           ├── p2p.rs              UDP binding, hole punching, QUIC, relay fallback
│           ├── udp_fallback.rs     plain-UDP socket for unusual network stacks
│           ├── session.rs          password authentication over QUIC
│           ├── color.rs            BGRA ⇄ I420 conversion (multi-threaded)
│           ├── host/
│           │   ├── mod.rs          online loop, ID/password, session handling
│           │   ├── stream.rs       capture → encode → send loop, flow control
│           │   ├── encoder.rs      OpenH264 encoder settings
│           │   ├── input.rs        SendInput mouse/keyboard injection
│           │   └── capture/
│           │       ├── dxgi.rs     DXGI Desktop Duplication
│           │       ├── gdi.rs      GDI BitBlt fallback
│           │       ├── win.rs      monitors, DIB canvas, cursor compositing
│           │       └── synthetic.rs  test pattern
│           └── viewer/
│               ├── mod.rs          connect flow + headless test mode
│               ├── window.rs       winit window, decoder thread, input capture
│               ├── decoder.rs      OpenH264 decoder
│               ├── render.rs       letterboxing + bilinear scaling
│               └── keymap.rs       physical keys → PC scan codes
```

---

## Building on Windows

### 1. One-time setup

This installs the Microsoft C++ build tools (OpenH264 and *ring* contain C/C++), Rust, and NASM (optional, for faster H.264 encoding). The build tools go first; otherwise the Rust installer installs the much larger Visual Studio Community instead. Run it in an **administrator** PowerShell or Command Prompt:

```bat
cd "C:\App Dev\PJDesk"
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --norestart --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
winget install --id Rustlang.Rustup -e
winget install --id NASM.NASM -e

```

Close the terminal and open a new one, so `cargo` is on the PATH.

### 2. Build

```bat
cd "C:\App Dev\PJDesk"
.\build.cmd

```

The first build takes a few minutes; later builds are incremental. You get `dist\pjdesk-server.exe` and `dist\pjdesk.exe`. Both are self-contained (no DLLs or runtime installs needed), so copy them to any Windows 10/11 PC.

Run the tests (optional):

```bat
cd "C:\App Dev\PJDesk"
cargo test --workspace

```

---

## Running

### Server

On the machine that will be the server:

```bat
cd "C:\App Dev\PJDesk\dist"
.\pjdesk-server.exe

```

(The `.\` prefix makes the commands work in both PowerShell and Command Prompt.)

It prints the ports, its data folder (`C:\ProgramData\PJDesk\server`) and its **key fingerprint**. When Windows Firewall asks, allow it. To install it as an **auto-start Windows service** instead (run as administrator; `--firewall` also adds the inbound firewall rule):

```bat
cd "C:\App Dev\PJDesk\dist"
.\pjdesk-server.exe service install --firewall

```

Other service commands are `service start`, `service stop` and `service uninstall`. The service logs to `C:\ProgramData\PJDesk\server\server.log`. Installing the service also limits the data folder, which holds the server's private key and the ID registry, to SYSTEM and Administrators.

**Ports:** TCP **and** UDP **21200** (change with `--port`). To use PJDesk over the Internet, forward both on your router to the server PC.

### Client

**Double-click `pjdesk.exe`.** The first time, it asks for your server's address and saves it. It then shares this computer (showing its **ID** and **password**) and lets you type another computer's ID to control it. Each viewer opens in its own window.

Or use the commands directly:

```bat
cd "C:\App Dev\PJDesk\dist"
.\pjdesk.exe config --set-server 192.168.1.10
.\pjdesk.exe host

```

And on the other computer:

```bat
cd "C:\App Dev\PJDesk\dist"
.\pjdesk.exe connect 123456789

```

| Command | What it does |
|---|---|
| `pjdesk host` | Share this computer. `--password X` sets a fixed password for unattended access. `--display N` picks a monitor. `--fps 30`, `--bitrate 6000` (kbit/s) and `--max-width 2560` (wider screens are downscaled) tune the stream. `--gdi` forces GDI capture and `--view-only` blocks control. |
| `pjdesk connect <ID>` | Control a computer. You're asked for the password (or pass `--password`). `--force-relay` skips P2P. |
| `pjdesk config` | Show settings. `--set-server ADDR[:PORT]`, `--fingerprint X` (pin the server key from its console), `--reset-fingerprint` (after reinstalling the server), `--reset-id` |
| `--server ADDR[:PORT]` | Use another server for one command |

**In the viewer:** your mouse and keyboard go to the remote computer. **Ctrl+Alt+Enter** toggles full screen. The small dot is your local pointer; the remote cursor is drawn in the video. Numpad digits arrive as digits, whatever the remote computer's NumLock state.

Client settings live in `%APPDATA%\PJDesk\client.json` (set `PJDESK_CONFIG` to use another file, for example to run two clients on one PC).

### Quick test on one PC

Open three terminals in `C:\App Dev\PJDesk\dist`:

```bat
cd "C:\App Dev\PJDesk\dist"
.\pjdesk-server.exe

```

```bat
cd "C:\App Dev\PJDesk\dist"
.\pjdesk.exe --server 127.0.0.1 host --password test1234

```

Replace `123456789` with the ID the host printed:

```bat
cd "C:\App Dev\PJDesk\dist"
.\pjdesk.exe --server 127.0.0.1 connect 123456789 --password test1234

```

(Controlling the same screen you're viewing makes a "hall of mirrors" effect; that's expected.)

---

## Backup server & moving the server

A PJDesk server's identity is a handful of files: its TLS key (which every
client pins), the user accounts and address books, and the computer list.
**Cloud Sync** keeps an encrypted copy of them in a Cloudflare R2 bucket (any
S3-compatible bucket works). A second server signed in to the same copy stays
in sync as a **standby**. You make it the **primary** when you want it to
take over.

**With a PJDesk account (easiest).** In the PJDesk Server window, open
**Cloud Sync & Backup Server** → **PJDesk account** and sign in (create the
account at pjdesk.com/account). The first server you sign in is the primary.
Sign in on another PJDesk Server with the same account and it becomes the
standby — nothing to copy between them. The password never leaves your servers:
they derive one key from it to sign in and another to encrypt the copy, so
PJDesk Cloud can't read it. (`pjdesk-server cloud signin --email you@example.com`
does the same from the command line.) See [cloud/README.md](cloud/README.md).

**Your own bucket (advanced).** Prefer to keep the copy in your own Cloudflare
R2 / S3 bucket? Choose **Your own bucket** instead:

**Set up (primary).** In Cloudflare, create an R2 bucket and an R2 API
token with *Object Read & Write* on that bucket. In the PJDesk Server window,
open **Cloud Sync & Backup Server**, choose *This is the main server*, paste
the S3 endpoint, bucket, access key ID and secret, and click **Start
syncing**. Then **Copy join code** and keep it somewhere safe (a password
manager) — it holds the bucket key and the encryption key.

**Add a standby.** Install PJDesk Server on another computer (another
location is the point), choose *This is a backup server*, paste the join
code, and click **Join**. It now has the same key fingerprint, accounts and
computer IDs, and keeps them current while on standby.

**Switch over** (planned move or outage). On the standby, choose **Make this
the primary** in the Cloud Sync panel or the tray, or run
`pjdesk-server cloud promote`. It downloads the latest copy first. The other
server becomes the standby the next time it checks, even if that server is
offline, so you do not stop it yourself. The primary can **Step down to
standby** (`pjdesk-server cloud demote`); that does not promote the other
server — you still choose **Make this the primary** there.

Computers connect to whichever server the DNS name points to (for example
`pjdesk.spader.us`). If the new primary is on a different network, forward
TCP + UDP 21200 to it and point that name at its public IP. A short TTL
(1–5 minutes) makes the change quick. Computers reconnect within a couple of
minutes (a client that hears nothing from its server for 70 s reconnects and
looks the name up again), keep their IDs, and need no changes of their own.

From the command line: `pjdesk-server cloud signin|signout|status|sync|promote|demote`
(PJDesk account) or `cloud setup|join|code|off|promote|demote` (your own bucket).
`--yes` skips the promote and demote prompt.

The copy is sealed with ChaCha20-Poly1305 under a random key made at setup.
The sync settings (bucket secret + key) are stored in the server's data
folder, readable by SYSTEM and Administrators only.

## Security model

| Layer | Protection |
|---|---|
| Client ⇄ server | TLS 1.3. The server's self-signed key is **trusted on first use** and pinned in `client.json`, and a changed key is refused loudly. For zero-trust setup, pin it up front with `pjdesk config --fingerprint <value from the server console>`. |
| Device IDs | Bound to a per-device secret; the server stores only its hash, so nobody can take over your ID. |
| Controller ⇄ host | QUIC (TLS 1.3) end-to-end. The host's certificate fingerprint is pinned, both direct and relayed. |
| Password | SPAKE2 PAKE with channel binding. The password never crosses the network. A man in the middle, even the server, gets **one online guess** per connection request and nothing to crack offline. |
| Brute force | Server: 30 connection requests per minute per IP. Host: one password guess per request and at most 3 requests waiting at once; a random password is replaced after 3 misses, and a fixed password locks out new sessions for 1 minute after 5 misses. |
| Relay | Forwards only for sessions the host accepted. Each side binds with its own token, and an address that is actively relaying one session can't be taken over by another. The relay sees only ciphertext. |
| Server data | The server's private key and the ID registry live in its data folder. Installing the service limits that folder to SYSTEM and Administrators. |

---

## What's tested

* **24 unit tests:** framing, UDP datagrams, SPAKE2 (right password, wrong password, MITM channel), TLS pinning, ID registry, rate limiting, color conversion, H.264 encode→decode round trip, flow control, scaling, key map.
* **End-to-end** with a real server, a host and a controller: direct P2P, forced relay, wrong password and offline ID, a wrong password followed by the right one, and a second controller being turned away while the first keeps streaming, plus a color check of the decoded test pattern.
* **Windows builds under Wine:** the server, a host with real GDI capture streaming a desktop, and the viewer window rendering it. Mouse moves landed on the exact pixel, and typing in the viewer put text into Notepad on the host.
* **Not yet tested on real Windows hardware:** DXGI capture, cursor compositing with real cursors, the Windows service, and the numpad-digit path. The first run on a real PC covers these.

---

## Troubleshooting

* **"no UDP reply from the server"**: UDP 21200 is blocked. Allow it in the server's firewall and forward it on the router.
* **Always "relayed"**: P2P is not possible between those networks (for example symmetric NAT or carrier-grade NAT). The relay still works, at the cost of server bandwidth.
* **"SERVER KEY MISMATCH"**: the server was reinstalled (new key), or something is intercepting. Compare with `pjdesk-server fingerprint`, then `pjdesk config --reset-fingerprint`.
* **Can't control admin windows or UAC prompts**: Windows blocks input from normal apps into elevated apps. Run `pjdesk.exe` as administrator on the host. UAC's secure desktop and Ctrl+Alt+Del need a SYSTEM service (roadmap).
* **Slow encoding at 4K**: use `--max-width 1920`, and build with NASM installed.
* **Verbose logs**: `set RUST_LOG=debug` before running.

---

## Limitations & roadmap

* Hardware H.264 encode/decode (Media Foundation: NVENC, Quick Sync, AMF) for 4K/60.
* Host as a SYSTEM service plus a per-session agent: control the login screen, UAC and Ctrl+Alt+Del, and survive sign-out.
* Clipboard sync, file transfer, multi-monitor switching, audio.
* Remote cursor shape as the local cursor (instead of compositing it into the video).
* GUI launcher (the console UI works today).
* TCP/WebSocket fallback for networks that block all UDP.
* IPv6 candidates.

---

## License

MIT, see [LICENSE](LICENSE). PJDesk compiles Cisco's **OpenH264** from source (BSD-2-Clause). Note that Cisco's free H.264 patent coverage applies only to Cisco's own prebuilt OpenH264 binaries; distributing your own build is your responsibility, which may matter if you ship it commercially.
