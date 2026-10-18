# RustDesk parity (PJDesk 0.4.0)

PJDesk is a self-hosted remote desktop: an app (`pjdesk`) and a hub
(`pjdesk-server`). This table is the RustDesk surface people actually use,
what 0.3.7 already did, and what 0.4.0 changes. A row marked **shipped** is
something you can use from the window or the command line. A row marked
**host ready** is implemented on the computer being controlled and covered by
tests or a Windows cross-compile, but the PJDesk viewer does not send it yet,
so it is not a working feature in this release. Those paths are left in the
tree so the next release can turn them on without another protocol change.

A 0.3.7 peer still gets a classic desktop session. Protocol 3 messages are
sent only after both sides agree. Account key derivation, device IDs, and the
sealed cloud-sync file list are unchanged.

## Remote session

| Feature | 0.3.7 | 0.4.0 | Notes |
| --- | --- | --- | --- |
| Desktop picture (H.264) | shipped | shipped | QUIC, direct or relayed. |
| Keyboard and mouse | shipped | shipped | |
| Special keys (Ctrl+Alt+Del, Win, Alt+Tab, lock, Task Manager) | shipped | shipped | Toolbar still sends these as input, plus Ctrl+Alt+Del as a special command. |
| Zoom, fit, fullscreen, refresh | shipped | shipped | |
| View-only | setting only | host ready | The host strips keyboard, mouse, clipboard, files, restart, privacy, block-input, and tunnels. The viewer has no view-only switch for a live session. |
| Per-permission grant | — | host ready | `Hello` negotiates a ceiling. The viewer does not send `Hello`. |
| Connection approval | — | host ready | A GUI host waits for a reply; a service host writes `consent.json`. The management window does not yet show that prompt. Unattended access still accepts a permanent password when policy allows it. |
| Unattended access | shipped | shipped | Permanent password, plus a server policy ceiling. |
| File transfer, both ways, resume | — | host ready | Writes only under a folder the host chose. Names are checked. Nothing received is executed. No drag-and-drop and no viewer panel. |
| Clipboard text | — | host ready | Refused in view-only. |
| Clipboard image | — | host ready | PNG on a side stream, capped. |
| Clipboard files | — | deferred | Windows `CF_HDROP` is a different channel and is not implemented. |
| Multi-monitor | — | host ready | The host can switch capture. The viewer shows one picture and has no monitor menu. |
| Audio (system loopback) | — | host ready | WASAPI capture and a player exist. The viewer never asks for audio. Microphone and camera are out. |
| Chat | — | host ready | |
| Session recording | — | deferred | An MP4 writer exists in the protocol crate and is tested. Nothing calls it from a session, so recording is not a feature. |
| Quality presets and adaptive bitrate | fps / bitrate settings | host ready | The host lowers bitrate when acks are late. The viewer does not send a preset. Codec stays H.264. |
| Privacy mode (black screen) | — | host ready | A cover window on the host. The viewer cannot turn it on. |
| Block local input | toolbar flag only | host ready | The toolbar toggle still only stops this window from sending input. It does not tell the host to block the local keyboard. |
| Keyboard mode (map / translate) | — | host ready | |
| Remote restart and reconnect | — | host ready | The host can restart Windows. The viewer does not request it or reconnect on its own. |
| Direct IP (no ID server) | — | host ready | The host listens on UDP 21201 and pins the certificate. The connect window has no address field. |
| LAN discovery | — | host ready | UDP beacon on 21202. No browse list in the window. |
| TCP tunneling | — | host ready | Loopback ports listed on the host, unless policy allows other hosts. No viewer control. |
| Wake-on-LAN | — | deferred | |
| Remote terminal / shell | — | deferred | A shell as SYSTEM is a larger risk than this release should take. |
| Printer, camera, microphone, whiteboard, plugins | — | deferred | |
| Web client | — | deferred | Would be a second client (WASM or a browser codec) and is not started. |
| Mobile clients | — | deferred | |
| IPv6 | — | deferred | |

## App window

| Feature | 0.3.7 | 0.4.0 | Notes |
| --- | --- | --- | --- |
| Share this computer (ID + one-time password) | shipped | shipped | |
| Connect by ID | shipped | shipped | |
| Address book (server account) | shipped | shipped | Synced through the account. Tags are stored on the entry and ignored by the window. |
| Recent and favorites | — | partial | Saved locally (`peers` in settings). The window does not list them yet. |
| Show / Hide on password fields | shipped | shipped | |
| Light and dark theme | dark | partial | A theme name is stored. The window still applies one theme. |
| Client auto-update | shipped | shipped | Pre-releases are skipped. A final release is newer than the same-numbered release candidate. |
| Install the background service | shipped | shipped | |

## Server

| Feature | 0.3.7 | 0.4.0 | Notes |
| --- | --- | --- | --- |
| ID, signaling, hole punching, relay | shipped | shipped | Ports 21200 TCP and UDP. |
| Accounts and address books | shipped | shipped | |
| Password rules for new server passwords | 8 characters | shipped | 12 characters, upper, lower, digit, symbol, no username, no leading or trailing space, a short common-password list. Existing passwords still sign in; a weak one is flagged in the login message. The app does not show that message on a successful sign-in yet. |
| PJDesk Cloud sign-in on the server | shipped | unchanged | pjdesk.com checks that password. The server does not apply the new rules to it. |
| Website account creation | shipped | shipped | The same rules run in the browser, before the key is derived. Sign-in on the site is not checked. |
| Roles (user, operator, admin) | admin flag | shipped (CLI) | `pjdesk-server account role`. The window still has the administrator checkbox. |
| Device groups | — | shipped (API) | Stored on the computer record. Set over the protocol or `group-requests.json`. No group column in the window. |
| Policy push | — | shipped (data) | Lives inside `accounts.json` (unattended, force prompt, permission ceiling, tunnel ports, recording). Pushed to hosts on protocol 3. No window or command edits it yet; the defaults apply. |
| Audit log | — | shipped (file) | `audit.log` in the data folder, rotated, no passwords. Not synced. The window does not open it. |
| Cloud Sync (account and R2) | shipped | shipped | |
| Make this server the primary / step down | — | shipped | Window, tray, and `pjdesk-server cloud promote` / `cloud demote`. A generation number and `If-Match` keep two servers from both writing. |
| Server auto-update | — | shipped | Off until you turn it on. See the changelog. Standby first, then primary, or 24 hours. |
| Server tray | — | partial | Open, check for updates, promote, step down, sync, exit. Close and minimize hide the window. Not in this release: start/stop/restart from the tray, open the log folder, a host count in the tooltip, one tray per user, or starting the tray at sign-in. |
| Web console, 2FA, OIDC, LDAP, custom client builder | — | deferred | RustDesk Pro features. Out of scope for a self-hosted build that keeps the current account format. |

## Left out on purpose

* **Clipboard files, web client, extra codecs (NVENC, AV1, H.265), mobile, printer, camera, microphone, remote shell, plugins, whiteboard, IPv6, and a full remote file browser.** Each is a separate product surface. Shipping a button that fails halfway is worse than leaving it out.
* **The viewer controls for protocol 3.** The host side is in place so a later viewer can use the same messages. Turning the buttons on without a Windows session to click through them would ship a half-working remote desktop. This release does not pretend those buttons exist.
* **Code signing.** Still a manual step; the updater keeps requiring `SHA256SUMS`.
* **Automatic failover when the primary is merely offline.** After the first promote, the generation is what decides who writes. The other server becomes primary only when someone promotes it, including while the old primary is off.

## What Ben does by hand

* Redeploy the website (`cloud\deploy.cmd`) so account-mode Cloud Sync honors `If-Match` and `/api/sync/health`. An own-bucket (R2) server already honors `If-Match` and does not need that deploy for fencing. Health reports in account mode do need it; until then a primary treats “no standby report” as “update whenever the window allows.”
* Keep publishing `SHA256SUMS` next to `PJDesk-Server-Setup-<version>.exe`.
* Re-seal unattended service passwords after the security fixes in 0.3.7, if that has not been done.
* Both sides need 0.4.0 before any protocol 3 session behavior applies. 0.3.7 still connects for keyboard and mouse.
* Forward UDP 21201 and 21202 only if you use direct IP or LAN discovery.
* Pick a receive folder on the host before file transfer can write anything.
* A host running as SYSTEM still needs someone at the screen to approve a file transfer.
* Server auto-update is off. Upgrading removes the old always-on “PJDesk Server Update” task. Turn the setting back on if you want it.
* A 0.3.7 server that uploads a cloud copy drops the generation field. Fencing starts again the next time a 0.4 server is promoted.
