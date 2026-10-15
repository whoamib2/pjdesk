# Changelog

## 0.2.0

The project is now **PJDesk** (formerly DeskLink), with a graphical app, a
background service, user accounts and Windows installers. Update the server and
all clients together — the signaling protocol changed (version 2).

### Apps

* **A real window.** Double-clicking `pjdesk.exe` opens a RustDesk-style
  window: your ID and password on the left, a box to connect to another
  computer on the right, your saved computers below, and a Settings window
  (General, Security, Network, Display, About). The old text mode is still
  there as `pjdesk console`.
* **Server management window.** Double-clicking `pjdesk-server.exe` opens a
  window with the service status, the server key fingerprint, user accounts
  (add/remove/disable) and the registered computers.
* **Viewer toolbar.** A control bar drops down at the top of a session: full
  screen, actual size, refresh, **Ctrl+Alt+Del**, block input, and disconnect.

### Background service

* **Reach this computer at the sign-in screen.** Install the service (from the
  installer, the Settings ▸ Security tab, or `pjdesk service install`) and
  PJDesk runs from boot. It launches a helper as SYSTEM into the active
  session and follows the input desktop, so you can connect before anyone
  signs in and through UAC prompts, including Ctrl+Alt+Del.

### Accounts

* **Sign in to your server.** Create accounts on the server
  (`pjdesk-server account add`, or the server window). Signing in from the app
  saves your login (encrypted with DPAPI) and syncs an **address book** of
  saved computers to every machine you use. Each computer can remember its
  password.

### Installers

* **One-click setup.** `PJDesk-Setup` and `PJDesk-Server-Setup` install to
  Program Files, add shortcuts and an uninstaller, and can set up the service.

### Security

* Account passwords are stored as Argon2id hashes; a saved login is a random
  token, revocable by changing the password. Login timing doesn't reveal
  whether an account exists. Saved passwords and tokens are encrypted at rest
  with DPAPI. The service folder is locked to SYSTEM and Administrators.

## 0.1.1

Fixes from a code review of 0.1.0. The wire protocol is unchanged, but update the server and all clients together.

### Security

* **One password guess per connection request.** A controller could previously open several QUIC connections within one request and try a password on each.
* **Pending requests are capped.** The host handles at most 3 unanswered requests at once, so strangers who know an ID can't keep it busy with requests they never finish.
* **Only one controller at a time.** If two controllers authenticate at the same moment, the second is turned away and the first keeps streaming.
* **Separate UDP tokens for host and controller,** so neither side can bind to the relay as the other.
* **Relay routes can't be hijacked.** A bind from an address that is actively relaying another session is ignored.
* **Server data folder locked down.** The service limits `C:\ProgramData\PJDesk\server` (private key, ID registry and log) to SYSTEM and Administrators when it is installed and each time it starts. On Linux the private key file is created with mode 0600.

### Host

* **DXGI capture recovers by itself.** It follows resolution changes, treats a temporarily unavailable desktop (UAC prompt, lock screen, session switch) as a pause instead of an error, and moves back from the GDI fallback to DXGI when it becomes available again.
* **GDI capture** follows resolution and monitor-layout changes, and no longer makes the host's mouse cursor flicker.
* **Keyboard and mouse:** keys and buttons are tracked by what Windows actually accepted, so "release everything" can't leave a key stuck. Pause, NumLock and PrintScreen, whose scan codes don't survive `SendInput`, are sent as virtual keys. Numpad digits arrive as digits whatever the host's NumLock state.
* **Older Windows:** the per-monitor DPI API is looked up at run time, so `pjdesk.exe` also starts on Windows 10 1607 and Windows Server 2016, where it falls back to the Windows 8.1 DPI API.

### Server

* **The Windows service** closes its sockets before reporting "stopped", so a restart can bind right away. It reports an error code to Windows when the server fails, and a relative `--data-dir` is resolved at install time.

### Viewer

* Mouse positions are exact to the pixel at the right and bottom edges.
* Keyframe requests after a decoding error are rate-limited, and every frame is acknowledged even if it fails to decode, so the stream can't stall.
* `pjdesk connect 123 456 789` accepts the ID with spaces, as the host displays it.

### Docs

* Commands use the `.\` prefix, so they work in PowerShell as well as Command Prompt.

## 0.1.0

First release: signaling server with ID registry and UDP relay (console or Windows service), and one client that both shares and controls, with P2P over QUIC, SPAKE2 passwords, DXGI/GDI capture, OpenH264 streaming and SendInput control.
