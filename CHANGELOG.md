# Changelog

## Unreleased

* Android 0.6.2. When someone connects is a pair of radio choices, and Use the permanent password turns on only after a password is saved. That choice is stored and the host honors it. Home shows Set permanent password. After Allow, the phone waits for a fresh screen-share consent and keeps the capture service in the mediaProjection type before the virtual display is created on the main thread. The status line says Ready, Waiting for approval, Waiting for screen sharing, Being controlled by a name, or Ended with the reason. The accessibility service does not request the floating shortcut button. Passwords are not written to the log.
* Android 0.6.1. The debug build no longer installs a second launcher icon. Home and Settings have Set permanent password (confirm field, Show/Hide, at least 8 characters) and Accept with password without asking. A Windows connection asks for screen sharing when you tap Allow, keeps the session up after that consent, and shows the real reason if it ends. Logs are tag `PJDesk` and a file you can share from Settings. The Windows viewer writes the same failures to `%APPDATA%\PJDesk\pjdesk.log` and does not ask for the password again unless it was wrong.
* Android app (`android/`, version 0.6.1) is a host and a controller. It shares `pjdesk-proto` and the relay through `pjdesk-link` and `pjdesk-mobile`. Video stays H.264 Annex-B: MediaCodec encodes on Android, and the Windows client keeps decoding with OpenH264. Android hosts accept taps, swipes, pinch, Back, Home, Recents, volume, and typed text. The Windows viewer maps mouse and scroll onto those gestures when the remote reports `android`.

## 0.5.2

* The PJDesk Server window lists each computer's version. A client that connected before 0.5.2 shows "unknown (<0.5.2)". A computer behind the current release on pjdesk.com gets an "update available" badge next to Update. This server's version is shown on the service card. The other Cloud Sync server's version comes from that server's health report. The unelevated status pipe carries the same device list and the other server's version.
* pjdesk.com `/admin/` is a dashboard: Overview, Accounts, Servers & devices, Plans, and an audit log. Plans are Free and Cloud Sync (the site does not advertise other prices). Granting a plan records who, when, the old plan, and the new plan. An expiry turns Cloud Sync off the next time that account is used. Server versions are stored from the sync health report. Desktop client versions stay on the PJDesk Server window, because the cloud copy is sealed. Local user accounts in the PJDesk Server window are not pjdesk.com plans.
* The client has a Saved computers page: search by name, ID, or operating system, sort, favorites, and a menu to rename, transfer a file, favorite, forget a password, or remove. The host reports its operating system during the session. Older hosts show an unknown icon. Operating-system marks are simple drawn shapes (a four-pane window, a laptop, a terminal, a phone, a monitor with a question mark), and the name and ID columns are left-aligned. A password is saved only after the other computer accepts it, with Windows DPAPI for the current user. The Remember box starts checked only when that option is on in Settings.
* A remote session can switch displays without reconnecting. The host sends each display's name, size, position, and DPI, and sends the list again when a display is added or resized. When the remote has more than one display, a tab strip across the top of the session window lists each screen and All screens. The tab in use is highlighted, and one click switches immediately. Ctrl+Alt+1 through 9 does the same. Separate windows stays available as a button at the end of that strip. One screen captures only that display. All screens is one capture of the whole virtual desktop, so the pointer crosses from one display to the next. Separate windows opens one local window per display, titled with the computer and the screen number. Each window can be dragged onto any local monitor and has its own fullscreen (a button, and F11) on the monitor it is sitting on. The session toolbar (files, chat, keys, End) stays on one window; every screen window has a short bar to go fullscreen or close that screen. Closing a window stops that display's capture and leaves the session up. Closing the last window, or End, ends the session. A display unplugged on the host closes its window and shows a short notice. The last view mode, the last display, and the window positions are remembered per computer and put back when the same local monitors are still attached. Ctrl+Alt+1 through 9 focuses that screen (or switches to it in the other modes), and Ctrl+Alt+Left/Right cycles.

## 0.5.1

* The server window, opened without Administrator rights, no longer pretends the data folder is empty. The running service publishes the fingerprint, update setting, last check, cloud role, signed-in email, and generation on a local pipe the interactive user can read. Sections the pipe does not cover say "Locked — open as administrator to view" instead of a blank sign-in form. Restart as administrator is a button. Icons are drawn, and the window uses the same type and theme as the client.
* An update no longer leaves a computer offline. The installer still stops the service and closes the program so it can replace the exe. The new program then starts the service again, retrying for a few seconds, and starts PJDesk in the signed-in user's session. The server update does the same for its service and tray. A second check runs if the service exits immediately. The result is written to the update log. The installer does not touch the server key, accounts, cloud sign-in, or update settings.

## 0.5.0

* The client window is a new layout: a left rail for this computer (ID, one-time password, status) and a main column to connect, with Recent, Favorites, Address book, and Discovered lists. Settings is a centered page with a sidebar, a scrolling body, and a fixed footer, so labels and Save stay on screen. Light and dark themes follow Windows unless you pick one. Icons are drawn shapes, so copy, show, refresh, and the check mark are no longer empty boxes. The in-session toolbar floats, collapses, and shows a tooltip on each button.
* Opening Settings no longer waits on Windows. The scheduled-task query (`schtasks`) and the service-status query used to run on the UI thread inside the click, which is why the window sat there. Those reads now happen on a background thread. The window paints from the last answer and shows a spinner until it arrives.
* A permanent password is still at least 8 characters. An account password is still the server's rule: 12 characters with upper and lower case, a digit, and a symbol. The hints say which is which.
* Transfer file connects the same way as Connect and opens the viewer's file panel.

## 0.4.1

* The Cloud Sync panel's backup-server note, and the README section **Backup server & moving the server**, describe the current switch: sign in on another PJDesk Server with the same account (it becomes the standby), then choose **Make this the primary** there, or use the tray, or `pjdesk-server cloud promote`. Computers still reach whichever server `pjdesk.spader.us` points to, so DNS and the port forward have to move if the new primary is on a different network.
* The installers read the workspace version from `Cargo.toml` when none is passed. `installer/build.sh` and both NSIS scripts no longer fall back to 0.2.0.
* Unused imports and unused code the compiler warned about are removed. What the programs do is unchanged.

## 0.4.0

Protocol 3. A 0.3.7 peer still connects for a classic desktop session (keyboard
and mouse). Account key derivation, device IDs, and the sealed cloud-sync
format are unchanged. The cloud bundle gained an `epoch` field that older
servers ignore. What you can click in 0.4.0, and what is only prepared on the
host, is listed in `docs/RUSTDESK-PARITY.md`.

### Session (protocol 3)

* The computer being controlled accepts the new messages when the other side
  speaks protocol 3: a permission grant, file transfer in both directions
  (resume, a folder you choose, nothing is executed), text and image clipboard,
  chat, system-audio loopback, monitor choice, quality with adaptive bitrate,
  a privacy screen, blocking local input, keyboard mode, restart, and TCP
  tunnels to ports you list. View-only and the server policy are ceilings.
  Someone at the computer is asked before a session starts, unless unattended
  access is allowed.
* The viewer toolbar (View, Keys, Session, Files, Chat, End) sends those
  controls. Files shows progress in both directions and resumes a partial
  download. Chat, clipboard text and image, audio, monitors, quality, the
  privacy screen, blocking the remote keyboard, and restart are on the bar or
  the side panel. Restart asks twice, then the viewer reconnects. A direct
  address and **Find on this network** are on the connect panel. The host's
  permission grant is what decides which of these actually run.
* Session recording writes an MP4 under `Videos\PJDesk`, using the size of the
  picture. The Files list is the host's receive folder, not the whole disk.

### Cloud Sync: make this server the primary

* On a standby, **Make this the primary** (Cloud Sync panel, tray, or
  `pjdesk-server cloud promote`). It works whether or not the current primary
  is online. Confirm explains that the other server becomes the standby.
* The primary can **Step down to standby** (`pjdesk-server cloud demote`).
* `--yes` skips the prompt. If the service is running, the command asks it to
  do the switch. If it is not, the command does the switch itself.
* Promote downloads the latest copy, applies it, adds one to a generation
  number stored in the sealed copy, and writes that copy with this server's
  id. Later uploads send `If-Match` (or `If-None-Match: *` when the object is
  missing) and are skipped when the cloud generation is newer or already
  belongs to another server. Two servers do not both write.
* The previous primary notices the newer generation on its next check, stops
  uploading, switches itself to standby, applies the cloud copy, and logs a
  notice. The Cloud Sync panel, tray tooltip, and `cloud-status.json` show
  the role, the generation, and which server is active.
* Copies from 0.3.7 have no generation. Until someone promotes, those servers
  keep the old "whoever has computers connected uploads" behavior.
* PJDesk Cloud honors `If-Match` / `If-None-Match: *` and still accepts an
  unconditional upload from an older server. It also stores a small health
  record (`/api/sync/health`) so a primary can see a standby's version.
  **Redeploy the website** (`cloud\deploy.cmd`) or account-mode fencing and
  the standby health report stay on the old Worker. An own-bucket (R2) server
  does not need that deploy: R2 already honors `If-Match`.

### PJDesk Server updates

* **Install server updates automatically** is off until you turn it on (server
  window, or `pjdesk-server update-task`). `update-task --disable` turns it off.
* `pjdesk-server update` checks and installs a newer release.
  `pjdesk-server update --check` only reports. The tray has **Check for updates now**.
* The check uses `https://pjdesk.com/api/release`, then GitHub if that is
  unreachable. Pre-releases are skipped. An older version is never installed.
* The installer is still verified with `SHA256SUMS` in a private directory,
  and hashed again immediately before it is launched. The server installer
  stops and restarts the service and does not delete the data folder. If the
  install fails after the service was stopped, the installer starts it again.
* While the setting is on, the service checks about once an hour (30–60
  minutes, different on each server). A standby may update first. A primary
  that can see a standby waits until that standby reports the new version and
  the current generation, or until 24 hours pass, so both do not restart
  together. An optional maintenance window defaults to any time.
* The window shows the last check, the last result, and an available version.
  A failed check or a failed launch leaves the service running.
* The installer no longer registers the old always-on "PJDesk Server Update"
  task. Upgrading removes that task. Turn updates on again if you want them.

### Accounts

* A new or changed server account password must be at least 12 characters, with
  an uppercase letter, a lowercase letter, a digit, and a symbol. It must not
  contain the username, start or end with a space, or match a short list of
  common passwords. The window shows a checklist and a strength meter while
  you type. An older weak password still signs in. The app then opens
  **Change your password** and shows the server's reason if the new one is refused.
* This does not apply to the PJDesk Cloud sign-in on the server (pjdesk.com
  checks that password). One-time host passwords are unchanged.

### Server window

* The tray starts when you sign in to Windows. That is on unless you turn it
  off. The installer writes the Run key and starts the icon, so it is there
  after install and after the next sign-in. A second launch signals the icon
  that is already running instead of adding another.
* The menu can open the window, start, stop, or restart the service (Windows
  asks for administrator), check for updates, open the log folder, promote,
  step down, sync, or exit. The tooltip names the service, how many computers
  are connected, the cloud role, and the update status. Closing or minimizing
  the window leaves the tray running.

## 0.3.7

Security fixes from a full review. The wire protocol is still version 2, and the
pjdesk.com account key derivation is unchanged, so existing 0.3.5 and 0.3.6
installs keep their IDs, accounts, and saved sign-ins. See `SECURITY-AUDIT.md`
for the findings and for the steps that have to be done by hand (redeploy the
website, keep publishing `SHA256SUMS`, re-seal an existing unattended service
password).

### Server and client

* The server data folder and the client service folder are limited to SYSTEM
  and Administrators. The signed-in console user can still read the service
  folder, so the window can show the one-time password. Other local accounts
  cannot read the server key, account tokens, address-book passwords, or the
  one-time password, and cannot drop a request file the service would obey.
* Request files only accept a device ID, `*` for "update all", or `now` for a
  sync or reload.
* Login tokens are stored as a SHA-256 hash. Old tokens are rewritten the next
  time the server starts. Clients still send the raw token.
* A wrong password, a missing account, and a disabled account all take the
  same time and return the same error. New passwords must be at least 8
  characters. Existing ones still work.
* Password checking no longer holds the lock that every other connection needs.
* A permanent password locks out new sessions for five minutes after five
  misses. The one-time password still rotates every three misses.
* Open connections, sessions, and relay traffic are capped so one address
  cannot fill the process or the relay.
* Automatic update requires `SHA256SUMS`, writes the installer into a private
  directory under a random name, and checks the bytes again before running
  them. A download URL that is not on GitHub is refused. The installers are
  still not code-signed.
* The unattended service password is sealed so the sign-in-screen helper can
  read it, and the settings window passes it through a file instead of the
  command line. An old user-scoped blob is re-sealed the next time someone
  who can open it runs the helper.
* Installers stop Windows from loading DLLs out of the folder they were
  launched from.
* Every password field in the client and server windows has a Show/Hide control.

### Website

* Sessions expire after 90 days idle or one year from creation. Sign-in
  attempts are limited per network as well as per email. Creating an account
  no longer says whether the address was already taken. Sync downloads are
  capped at 8 MB. Pages send a content security policy and the account and
  admin scripts are files, not inline.

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
