# RustDesk parity (PJDesk 0.4.0)

PJDesk is a self-hosted remote desktop: an app (`pjdesk`) and a hub
(`pjdesk-server`). This table is the RustDesk surface people actually use,
what 0.3.7 already did, and what 0.4.0 changes. A row marked **shipped** is
something you can use from the window or the command line. A row marked
**host ready** is implemented on the computer being controlled, but the viewer
does not offer a control for it yet.

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
| View-only | setting only | shipped | The host strips keyboard, mouse, clipboard, files, restart, privacy, block-input, and tunnels. The viewer keeps those controls disabled for that session. There is no separate "watch only" button on the viewer. |
| Per-permission grant | — | shipped | The viewer sends `Hello` as soon as a protocol 3 session starts. The host grants only what its policy and the person at the computer allow. |
| Connection approval | — | host ready | A GUI host waits for a reply; a service host writes `consent.json`. The management window does not yet show that prompt. Unattended access still accepts a permanent password when policy allows it. |
| Unattended access | shipped | shipped | Permanent password, plus a server policy ceiling. File transfer is not included in that default grant. |
| File transfer, both ways, resume | — | shipped | Files panel, or drop a file on the viewer. Uploads go to the folder the host chose. Downloads go to `Downloads\PJDesk` and resume a partial file. Names are checked. Nothing received is executed. |
| Clipboard text | — | shipped | Both ways, after the first sample, and only when the host allowed it. |
| Clipboard image | — | shipped | Send clipboard, or a PNG side stream when the image is large. Capped. Refused when clipboard is not allowed. |
| Clipboard files | — | deferred | Windows `CF_HDROP` is a different channel and is not implemented. |
| Multi-monitor | — | shipped | Session panel lists the host's monitors. The picture is the one you pick. |
| Audio (system loopback) | — | shipped | Session menu, when the host allowed audio. Microphone and camera are out. |
| Chat | — | shipped | Chat panel in the viewer. On the computer being shared, chat shows in the PJDesk window only when that window is the host, not when the background service is sharing. |
| Session recording | — | shipped | Session menu. The file is an MP4 under `Videos\PJDesk`, sized to the picture. The host can turn recording off in policy. |
| Quality presets and adaptive bitrate | fps / bitrate settings | shipped | Session menu: smooth, balanced, sharp, high. The host still lowers bitrate when acks are late. Codec stays H.264. |
| Privacy mode (black screen) | — | shipped | Session menu, when the host allowed it. |
| Block local input | toolbar flag only | shipped | Session menu **Block remote input** tells the host to ignore its own keyboard and mouse. This window can still control it. |
| Keyboard mode (map / translate) | — | host ready | The host understands both. The viewer still sends scan codes. |
| Remote restart and reconnect | — | shipped | Session menu, confirm by choosing it again. The viewer waits and reconnects. Needs the restart permission. |
| Direct IP (no ID server) | — | shipped | Connect panel address field, and `pjdesk connect --direct host`. The certificate is pinned on first use. `--trust` replaces a changed pin. |
| LAN discovery | — | shipped | **Find on this network** lists beacons and connects to the sender's address on UDP 21201. |
| TCP tunneling | — | shipped | Session panel. The viewer listens on localhost and the host connects only to a listed port (loopback unless policy says otherwise). |
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
| Password rules for new server passwords | 8 characters | shipped | 12 characters, upper, lower, digit, symbol, no username, no leading or trailing space, a short common-password list. An older weak password still signs in, and the app then asks you to change it. A rejected new password is shown in the window. |
| PJDesk Cloud sign-in on the server | shipped | unchanged | pjdesk.com checks that password. The server does not apply the new rules to it. |
| Website account creation | shipped | shipped | The same rules run in the browser, before the key is derived. Sign-in on the site is not checked. |
| Roles (user, operator, admin) | admin flag | shipped (CLI) | `pjdesk-server account role`. The window still has the administrator checkbox. |
| Device groups | — | shipped (API) | Stored on the computer record. Set over the protocol or `group-requests.json`. No group column in the window. |
| Policy push | — | shipped (data) | Lives inside `accounts.json` (unattended, force prompt, permission ceiling, tunnel ports, recording). Pushed to hosts on protocol 3. No window or command edits it yet; the defaults apply. |
| Audit log | — | shipped (file) | `audit.log` in the data folder, rotated, no passwords. Not synced. The window does not open it. |
| Cloud Sync (account and R2) | shipped | shipped | |
| Make this server the primary / step down | — | shipped | Window, tray, and `pjdesk-server cloud promote` / `cloud demote`. A generation number and `If-Match` keep two servers from both writing. |
| Server auto-update | — | shipped | Off until you turn it on. See the changelog. Standby first, then primary, or 24 hours. |
| Server tray | — | shipped | Starts at Windows sign-in (on by default; the installer writes the Run key and starts it). One icon per user. Menu: open, start/stop/restart the service (UAC), check for updates, open the log folder, promote, step down, sync, exit. Tooltip: service, connected computers, cloud role, update status. Close and minimize hide the window. |
| Web console, 2FA, OIDC, LDAP, custom client builder | — | deferred | RustDesk Pro features. Out of scope for a self-hosted build that keeps the current account format. |

## Left out on purpose

* **Clipboard files, web client, extra codecs (NVENC, AV1, H.265), mobile, printer, camera, microphone, remote shell, plugins, whiteboard, IPv6, and a whole-disk remote file browser.** The Files panel lists only the folder the host chose for received files. A browser of the rest of that disk is not included.
* **Keyboard translate mode, and a viewer button that requests a view-only session.** The host already enforces both when they are set on that computer.
* **Chat while the background service is the host.** The person connecting can chat. The PJDesk window on that computer shows chat only when the window itself is sharing.
* **Code signing.** Still a manual step; the updater keeps requiring `SHA256SUMS`.
* **Automatic failover when the primary is merely offline.** After the first promote, the generation is what decides who writes. The other server becomes primary only when someone promotes it, including while the old primary is off.

## What Ben does by hand

* Redeploy the website (`cloud\deploy.cmd`) so account-mode Cloud Sync honors `If-Match` and `/api/sync/health`. An own-bucket (R2) server already honors `If-Match` and does not need that deploy for fencing. Health reports in account mode do need it; until then a primary treats “no standby report” as “update whenever the window allows.”
* Keep publishing `SHA256SUMS` next to `PJDesk-Server-Setup-<version>.exe`.
* Re-seal unattended service passwords after the security fixes in 0.3.7, if that has not been done.
* Both sides need 0.4.0 before any protocol 3 session behavior applies. 0.3.7 still connects for keyboard and mouse.
* Forward UDP 21201 and 21202 only if you use direct IP or LAN discovery.
* Pick a receive folder on the host before a file can be uploaded to it. Downloads from that folder land in `Downloads\PJDesk` on the computer you are connecting from.
* The tray Run key is written for the Windows user who runs the installer. Other users on that PC do not get an icon until they open PJDesk Server once (the window then turns sign-in startup on).
* A host running as SYSTEM still needs someone at the screen to approve a file transfer.
* Server auto-update is off. Upgrading removes the old always-on “PJDesk Server Update” task. Turn the setting back on if you want it.
* A 0.3.7 server that uploads a cloud copy drops the generation field. Fencing starts again the next time a 0.4 server is promoted.
