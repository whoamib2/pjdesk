# PJDesk security audit (0.3.7)

This is a review of the PJDesk code as it stood on `main` before this change, and of the fixes that landed with version 0.3.7.

## For someone who is not a security specialist

PJDesk lets one Windows computer control another. The server that introduces them, the program that shares the screen, and the website at pjdesk.com all have to keep strangers out. The review found two problems that were serious on a shared Windows PC, and a set of smaller ones.

The serious pair were both about files on the computer itself. The server stores its private key, the list of accounts, and saved computer passwords in `C:\ProgramData\PJDesk\server`. The client service stores the one-time password you read out to someone in `C:\ProgramData\PJDesk\service`. Those folders were readable, and in the server's case also writable, by every account on the machine. A person who could sign in locally — a guest, a standard user, a piece of malware running as that user — could read the key and the passwords, and could also drop a small file that the server service would obey. That file could tell every connected computer to install an update, forget a computer, or reload accounts. The service runs as Windows' most privileged account, so it did those things without asking.

Those folders are now limited to Windows itself and to Administrators. The signed-in person can still see their own one-time password. Nobody else on the PC can.

The other fixes make password guessing slower and less informative, stop a flood of connections from tying up the server, require a published checksum before an automatic update will install, and tighten the website (sessions expire, sign-in attempts are limited, pages tell the browser which scripts they will run).

A few things are left on purpose. The installers are still not code-signed, because there is no signing certificate yet; updates will not install unless the release includes a `SHA256SUMS` file. The website still cannot send email, so it cannot fully hide whether an address already has an account. The device identity is still derived from a Windows value any local user can read; changing that would give every existing computer a new ID.

**What you need to do after merging:** redeploy the website from your PC with `cloud\deploy.cmd`. No database migration. Keep publishing `SHA256SUMS` with every release. Sign in once on each computer that uses an unattended service password (or run `pjdesk service configure` again) so that password is re-saved in a form the sign-in screen can read. Details are at the bottom.

The password math for pjdesk.com accounts is unchanged. Existing accounts keep working.

## How to read the findings

Severity is about what an attacker gets, not about how clever the bug is.

- **Critical** — a local user, or anyone, can take over a computer or read the keys that protect every session.
- **High** — a realistic path to account takeover, a bad update, or a denial of service that stops the product.
- **Medium** — a real weakness that needs extra conditions, or that is now slowed down rather than removed.
- **Low** — narrow, noisy, or already bounded by something else.
- **Info** — looked at, recorded so the next change does not undo it. Not a vulnerability by itself.

"Fixed" means the code in this change closes it. "Accepted" means the design is intentional and the remaining risk is written down. "Deferred" means a real improvement that needs something this repository does not have (a code-signing certificate, an email sender, or a breaking change to existing device IDs).

## Critical

### C1. Server data folder was readable and writable by every local user

**Where:** `crates/pjdesk-server/src/service.rs` (`repair_data_dir`), and every file the server writes under the data folder (`accounts.json`, `devices.json`, `server-key.pem`, `cloud-sync.json`, address books, `update-requests.json` and the other request files in `crates/pjdesk-server/src/requests.rs`).

**Impact:** `%ProgramData%` lets Users read files and create files in new folders. The server used to reset its folder back to that inherited access. Anyone signed in on the server PC could read the TLS private key (and then impersonate the server to clients that had pinned it), the account file, address-book passwords, and the cloud-sync key. They could also create a request file. The service, running as SYSTEM, applies those files: update every online computer, forget a computer, or reload the TLS key and the accounts from disk.

An earlier attempt to lock the folder down applied inheritance flags to files. Windows treats those flags as invalid on a file and can leave the file with an empty access list, which locks out SYSTEM too and stops the service. The repair has to avoid that trap. The comment in the old code described the unsafe reset as the safe option. That comment is corrected.

**Fix:** On startup the folder is granted only to SYSTEM and Administrators, with inheritance set on the directory. Children are reset so they inherit that, and the directory itself is not reset (resetting it would put the Users access back). Each sensitive file is locked again when it is written. Request targets are checked: an update is a device ID or `*`, a forget is a device ID, a sync or reload is the word `now`. Anything else is ignored.

The management window already relaunches itself as administrator when it cannot write the folder, so this matches how the window was meant to be used.

### C2. The one-time password and the service settings were shared with every local user

**Where:** `crates/pjdesk/src/service.rs` (`secure_service_dir`), `crates/pjdesk/src/syspaths.rs` (`status.json`), `client.json` in the service folder.

**Impact:** The service folder was granted Modify to the Users group. `status.json` contains the current one-time password in clear text so the PJDesk window can show it. `client.json` contains the server address, the pinned server fingerprint, and the unattended password. Any local user could read the password and connect, or rewrite the config to point the computer at their own server and clear the pin.

**Fix:** The folder is granted to SYSTEM and Administrators, plus the account that is actually signed in at the console (so that person's window and the in-session helper can still read it). Other accounts on the PC cannot. The one-time password is no longer printed in the service log.

## High

### H1. Automatic updates could install a file that was swapped on disk, and a missing checksum was allowed

**Where:** `crates/pjdesk-proto/src/update.rs`. The scheduled task runs the installer as SYSTEM.

**Impact:** The checksum file was optional. The download used a predictable name in the shared temp directory. A local user can replace a file there, or plant a junction so the write lands somewhere else, in the gap between "checksum looked fine" and "SYSTEM ran the exe". A release that simply omitted `SHA256SUMS` was installed on HTTPS trust alone.

**Fix:** A release without `SHA256SUMS` is refused. The file is written under `ProgramData\PJDesk\updates` (or `~/.local/share/PJDesk/updates`), the directory is locked to SYSTEM and Administrators, a symlink or junction already there is removed, the name is random, the file is created only if it does not exist, and the bytes are read back and hashed again before the installer is started. Download URLs have to be HTTPS on `github.com` or a `githubusercontent.com` release host, so a tampered release listing cannot send the updater somewhere else.

**Still deferred:** the installers are not code-signed. A checksum published next to the installer detects a truncated or swapped download. It does not detect a release that an attacker published from the GitHub account itself, or a TLS interception that rewrote both the installer and the checksum. That needs a signing certificate. Requiring a signature now would stop every update, including this one. See D1.

**Compatibility:** 0.3.6 still updates to 0.3.7, because 0.3.6 does not require `SHA256SUMS`. From 0.3.7 on, a release that forgets that file will not auto-install. Publish it every time.

### H2. Sign-in told you whether an account existed, and skipped the slow check for some of them

**Where:** `crates/pjdesk-server/src/accounts.rs` (`login_password`, `login_token`).

**Impact:** A disabled account returned immediately, before the password hash was checked, and the error text differed from a wrong password. A token sign-in used different text for "no such user" and "bad token". Both let someone scan for real usernames. The fast path also made disabled accounts obvious from timing.

**Fix:** A missing username is checked against a dummy hash so the wait is the same. A disabled account is still checked against its real hash, then refused with the same "wrong username or password" text. Token sign-in uses one message for an unknown user and a bad token. New passwords must be at least 8 characters. Existing stored hashes are not touched.

### H3. Login tokens were stored in the account file as the client sends them

**Where:** `crates/pjdesk-server/src/accounts.rs`.

**Impact:** `accounts.json` held the raw token. Anyone who could read that file (C1) could sign in as that user until the token expired, without knowing the password.

**Fix:** The file now stores `h:` plus the SHA-256 of the token. Clients still send the raw token. The next time the server loads the file it rewrites old tokens and saves. A key that already starts with `h:` is left alone.

### H4. Checking a password stalled every other connection

**Where:** `crates/pjdesk-server/src/server.rs` (`handle_login`).

**Impact:** The server keeps its whole state behind one lock. Password checking (Argon2) ran while that lock was held. One sign-in, or a stream of them, stopped registrations, pings, and new sessions for everyone until the hash finished.

**Fix:** The rate limit and the account lookup happen under the lock. The hash check happens after the lock is released. The result is committed under the lock again, and it is refused if the account was disabled or the hash changed in between. Token sign-in stays under the lock; it is a map lookup, not a password hash.

### H5. The unattended password could not be read at the sign-in screen, and machine-wide encryption alone would have leaked it

**Where:** `crates/pjdesk/src/secure.rs`, `crates/pjdesk/src/service.rs` (`session_host`, `write_service_config`).

**Impact:** DPAPI blobs were bound to the user who set the password. At the lock screen and the sign-in screen the helper runs as SYSTEM, so it could not decrypt that blob and unattended access did not work there. Switching the blob to "this machine" without also locking the file would let every local user decrypt it.

**Fix:** New service passwords are sealed for the machine (`dpapi-machine:`), and the service folder ACL from C2 is what keeps other users out. The first time a helper runs as someone who can still open an old `dpapi:` blob, it re-seals it. SYSTEM cannot open an old user blob by itself, so a computer that is sitting at the sign-in screen with an old blob needs one sign-in, or `pjdesk service configure`, before unattended access works there again.

User-saved passwords in the signed-in person's own config stay bound to that user. That is the right scope for those.

### H6. Website sessions did not expire, sign-in was only limited per email, and pages had no content security policy

**Where:** `cloud/src/index.js`, `cloud/public/_headers`, `cloud/public/index.html`, `cloud/public/account/`, `cloud/public/admin/`.

**Impact:** A stolen Bearer token worked until the password was changed. Sign-in failures were counted per email, so one network could try many addresses. Creating an account returned "already exists", which lists who has an account. The sync download did not enforce the 8 MB cap the upload uses. There was no Content-Security-Policy, and the account and admin pages ran inline scripts, so a markup bug would have been able to run script.

**Fix:** Sessions die after 90 days idle or 365 days from creation (`created_at` was already in the database; no migration). Sign-in is also limited per IP (30 failures per 15 minutes, on top of 10 per email). Signup always answers the same success, including when the address is taken. Sync downloads over 8 MB are refused, and uploads are stopped while they are read, not after the whole body is buffered. Responses send `nosniff`, `no-referrer`, and `X-Frame-Options: DENY`. The static site sends HSTS and a CSP of `script-src 'self'`. Inline scripts moved into `site.js`, `account/account.js`, and `admin/admin.js`. The homepage version string is only written into the page when it looks like a version number. Release tags that would not be a single safe path segment are rejected before they become a redirect.

**Still accepted:** without email, a later sign-in with a password the caller just chose still reveals that they do not own an existing address (the sign-in fails). Closing that needs a mailer. None was added. `style-src` still allows inline style, because the homepage CSS is inline. Scripts do not. Google Fonts is still loaded; that is a privacy dependency, not an injection hole.

**Compatibility:** a server that stays signed in for a year will be asked to sign in again. Tokens in use today are not expired by the deploy.

## Medium

### M1. A connection flood could pin the relay or fill the process with handshakes

**Where:** `crates/pjdesk-server/src/relay.rs`, `crates/pjdesk-server/src/server.rs`.

**Impact:** A relay route is created when both sides have bound, which is before the password check, because the password check runs over the QUIC connection that may itself be relayed. An open-ended forward lets someone who can open sessions push bandwidth through the server. Separately, every accepted TCP connection spawned a task and started TLS with no cap.

**Fix:** Each relay direction stops at 1 GiB and the process keeps at most 4096 routes. Bind datagrams are limited to 30 per second per address before the session table is locked. Signaling is limited to 8 open sessions per controller address and 2000 overall, 30 connect attempts, 15 sign-in attempts, and 20 registrations per minute per address, and a connection that sends more than 40 signaling messages in a second is dropped. New TCP connections are refused past 1024 overall or 32 from one address, before the TLS handshake.

The relay still cannot see inside the QUIC packets. The byte cap limits damage; it does not replace the password.

### M2. A permanent password could be guessed online as fast as the network allowed

**Where:** `crates/pjdesk/src/host/mod.rs` (`record_failure`), `crates/pjdesk-server/src/accounts.rs` (`create`).

**Impact:** The one-time password already rotates every three misses, and each connection gets one guess. A permanent password did not. With the computer online, a guesser could try it continuously.

**Fix:** When a permanent password is set, five wrong passwords refuse new sessions for five minutes. New permanent passwords and new server-account passwords must be at least 8 characters. This slows guessing. It does not make a short or reused password safe.

### M3. The GUI put the unattended password on the elevated process command line

**Where:** `crates/pjdesk/src/platform.rs`, `crates/pjdesk/src/main.rs`, `crates/pjdesk/src/gui/settings.rs`.

**Impact:** Windows shows the command line of a process to other local users and writes it to audit logs. The settings window launched the elevated helper with `--password` on that command line.

**Fix:** The window writes the password to a file in the user's temp directory and passes `--password-file`. The helper reads it and deletes it. If the UAC prompt is dismissed, the window deletes the file after ten minutes. The service install no longer prints the password. `--password` still exists for the command-line tools; anyone using it should know it is visible in the process list.

### M4. A pasted S3 endpoint could change the request the server sent

**Where:** `crates/pjdesk-server/src/cloud.rs` (`normalize_endpoint`).

**Impact:** The endpoint from a join code or the setup form is placed into signed requests. A value with a newline, a username in the URL, or a scheme could aim those requests somewhere else or inject a header.

**Fix:** The host must be a hostname (letters, digits, dot, hyphen, and a port). Userinfo, spaces, and control characters are rejected.

### M5. A cloud copy dated far in the future, or a huge file inside it, could brick or exhaust a standby

**Where:** `crates/pjdesk-server/src/cloud.rs` (`apply`, `apply_bundle`).

**Impact:** A standby applies a copy only when its timestamp is newer than what it already applied. A copy dated years ahead would make every later genuine copy look old, so the standby would stop updating. A single enormous file in an otherwise valid sealed bundle would be written before any size check.

**Fix:** A copy more than 24 hours ahead of the standby's clock is refused. Each file is capped at 4 MB and the set at 8 MB, matching the website's upload limit. Paths were already limited to the known state files and `addressbook/<name>.json`. After a file is written, `.pem` and `.json` are locked down again. The temporary key check writes the incoming key and locks it before it is loaded.

**Residual:** someone who has the sync key can still replace the TLS key and the accounts. That is what a backup server is for. Someone who has the bucket but not the key cannot forge a copy. They can offer an older copy again. A standby that has already applied a newer one ignores it. A server that is joining for the first time cannot tell the latest copy from an old one the bucket put back. Confidentiality still holds. See A3.

### M6. The installers loaded DLLs from the folder they were launched from

**Where:** `installer/pjdesk-client.nsi`, `installer/pjdesk-server.nsi`.

**Impact:** A DLL sitting next to the installer can be loaded into the installer process, which is usually elevated.

**Fix:** Both the installer and the uninstaller call `SetDllDirectoryA("")` at startup, which stops Windows from searching the launch folder for DLLs.

## Low

### L1. The device secret is derived from a value every local user can read

**Where:** `crates/pjdesk/src/identity.rs`, `pjdesk_proto::derive_device_secret`.

**Impact:** The secret is SHA-256 of a fixed label plus the Windows MachineGuid. MachineGuid is in `HKLM` and readable by any signed-in user. That user can register as this computer from another address and take the online slot. The real copy is pushed to standby.

**Status:** Deferred. Changing the derivation would give every existing computer a new ID and break address books. The server now logs a warning when the same ID registers from a different IP. Treat a local user who can read MachineGuid as someone who can claim the ID. A later design can add a second secret that is not readable by Users, and keep the current one so old IDs still resolve.

### L2. `--password` on the command line is still visible

**Where:** `pjdesk service install --password`, `pjdesk-server account add`.

**Status:** Accepted for the command-line tools. The window no longer does this (M3). Prefer `--password-file`, or a prompt, when other people can see the process list.

### L3. Saved computer passwords sit in the address book as the server stores them

**Where:** `crates/pjdesk-server/src/accounts.rs` address-book files, and the same files inside the sealed cloud bundle. The comment in `crates/pjdesk-proto/src/signal.rs` used to say they were encrypted at rest. They are not, separately from the cloud seal.

**Status:** Accepted after C1. The server has to hand the password to the signed-in client, so it cannot be a value only the client can open unless the client keeps a key the server does not have. After the ACL fix, only SYSTEM and Administrators can read the files. Inside the cloud bundle they are protected by the sync key.

### L4. The website can still be used to guess which emails have accounts

**Where:** `cloud/src/index.js` signup and login.

**Status:** Accepted. Signup no longer says "already exists". A sign-in with a password the caller chose still fails for an address that already exists and succeeds only for one they just created. Fixing that properly means sending mail. No mail provider was added.

### L5. A malicious signaling server can ask hosts to update

**Where:** the host's handling of `UpdateNow` in `crates/pjdesk/src/host/mod.rs`, together with `update.rs`.

**Impact:** The server can tell a connected host to run the updater. The updater then talks to GitHub, not to the signaling server, and from 0.3.7 it requires `SHA256SUMS` and a GitHub download URL.

**Status:** Accepted. A hostile server can cause the latest published release to be installed. It cannot choose the bytes, unless it also controls the GitHub release (D1).

## Accepted or deferred, recorded so they are not "fixed" by accident

### A1. First connection to a server is trust-on-first-use

**Where:** `crates/pjdesk/src/signaling.rs`.

The server certificate is self-signed. The client stores the SHA-256 fingerprint on first connection and refuses a later change with a server-key mismatch. A first connection on a hostile network can be intercepted. After the pin is saved, a replacement key is shown as a mismatch rather than accepted. Users can pin a fingerprint they read from the server console with `pjdesk config --fingerprint`.

### A2. The password, not the server, is what lets someone control the screen

**Where:** `crates/pjdesk-proto/src/auth.rs`, `crates/pjdesk/src/session.rs`, `crates/pjdesk/src/host/stream.rs`.

Host and controller run SPAKE2 and then an HMAC over the TLS exporter `EXPORTER-pjdesk-auth-v1` for that connection. A relay or a server that terminates TLS with its own certificate gets a different exporter and the proof fails. One password guess is allowed per connection request. The host offers the one-time password and, if set, the permanent one. Mouse, keyboard, and Ctrl+Alt+Del are injected only after that succeeds, and view-only mode drops input. The screen is not sent before authentication.

Ctrl+Alt+Del from an authenticated controller is the feature that reaches the sign-in screen. It is not available before authentication.

### A3. Whoever holds the sync key can replace a standby's TLS key and accounts

**Where:** `crates/pjdesk-server/src/cloud.rs`.

That is the standby design: the backup becomes the server, key and all. The key is the password-derived sync key (PJDesk Cloud) or the random key inside a join code (your own bucket). The join code is a secret; it contains the bucket credentials and the encryption key. Store it like a password.

A bucket or a website that does not have the key cannot read or forge the copy. They can replay an older sealed copy. A standby that has applied something newer ignores it (`should_apply`). A brand-new join cannot tell a replay from the latest copy. Nonces are 96 random bits per seal; a collision is not a practical concern at this volume. The account key derivation is unchanged, including the published test vector, so existing accounts keep working.

Signature V4 for a customer's own bucket matches the AWS GET example and was not rewritten. `Content-Type` stays unsigned so existing buckets keep accepting uploads.

### A4. Installers are not code-signed

**Where:** release assets `PJDesk-Client-Setup-<ver>.exe` (also published as `PJDesk-Setup-<ver>.exe`) and `PJDesk-Server-Setup-<ver>.exe`.

**Status:** Deferred until there is a certificate. Checksums are required (H1). Do not turn on "reject unsigned" until releases are actually signed, or updates stop.

### A5. Console password prompts have no Show/Hide button

**Where:** `rpassword` prompts in `pjdesk` and `pjdesk-server`.

Every password field in a window has a Show/Hide control: the client (permanent password, connect, sign-in, service password), the server window (account password, cloud password, secret key, join code), and the website (`data-reveal` on every password input). A console prompt is masked and stays masked. There is no second control to add without building a new prompt.

### A6. Frame sizes and `unsafe` blocks

Signaling frames are capped at 64 KB, control frames at 256 KB, video headers at 4 KB, and video frames at 16 MB. Oversized lengths return an error. `unsafe` in this tree is Windows capture, input, DPI, DPAPI, and process creation. None of it decodes a network packet by pointer. Postcard decode failures return `Result`.

### A7. What is in the repository

`cloud/wrangler.toml` contains the Cloudflare account id, the D1 database id, and `ADMIN_EMAILS` (`ben@spader.us,pjdesk@spader.us`). Those are identifiers and the admin list, not API tokens. They were not changed. No API tokens or private keys were found in the tree. The SigV4 test uses Amazon's published example access key, which is not a live credential.

### A8. Dependencies

The Worker runtime is the single file `cloud/src/index.js`. It has no npm dependencies. `npm audit` of the local `wrangler` toolchain reported nothing. Those packages do not ship to pjdesk.com. `cargo audit` 0.22.2 reported no vulnerabilities in `Cargo.lock`. It did warn that four transitive crates are unmaintained or, in one case, unsound in an API this program does not call: `atomic-polyfill` and `proc-macro-error` (unmaintained), `ttf-parser` 0.25.1 (unmaintained; used to draw bundled UI fonts, not files from the network), and `glib` 0.18.5 (an unsound iterator, pulled in only by the Linux tray icon via GTK). None of those are fixed here. Bumping GTK or the font stack is a separate change, and it would not close any of the findings above.

D1 queries that include a value use bound parameters. Admin routes call `requireAdmin`. The API uses `Authorization: Bearer`, not cookies, so a browser on another site cannot attach the token. CORS is not opened.

## Later features, and what they must not do

File transfer and clipboard sync are not in this version. When they are added:

- Both must run on the authenticated QUIC session, after SPAKE2, never on the signaling connection. The signaling server is in a position to see signaling messages. It must not see files or clipboard bytes.
- The clipboard is an exfiltration path. A view-only session should not read or write it. A session that can write the clipboard can paste into a password field on the host.
- File transfer must write only under a directory the person at the computer chose, and must not follow junctions out of that directory. The sign-in-screen helper runs as SYSTEM. A transfer that writes or executes as SYSTEM, with no prompt on the secure desktop, is a remote admin shell. Gate it on the same consent as input, and do not run received files.
- A future "remember this device" feature must not weaken the one-guess-per-connection rule or store a reusable secret in a file Users can read.

## What Ben needs to do by hand

1. Redeploy the Worker from the PC that already has the Cloudflare login: `cloud\deploy.cmd`. This audit does not deploy it. There is no D1 migration. `ADMIN_EMAILS` is unchanged.
2. Keep publishing `SHA256SUMS` next to both installers on every GitHub release. From 0.3.7, automatic update refuses a release that does not have it.
3. On each computer that already has an unattended service password sealed the old way: sign in once so the helper can re-seal it, or run `pjdesk service configure` and set the password again. Until then, the sign-in screen cannot read the old blob. After an upgrade, the new ACL is applied the next time the service starts.
4. Plan on a code-signing certificate before treating update integrity as done (A4).
5. A cloud session that is idle for 90 days, or that was created more than a year ago, will need a sign-in. Nothing expires at the moment of deploy.
6. Do not rotate the account key derivation. It was not changed.

## Compatibility

- Protocol version stays 2. Old 0.3.5 and 0.3.6 clients and servers still speak to each other.
- Account key derivation is unchanged. Test vector: email `Test@Example.com ` / password `correct horse battery staple` → auth `42616cb9b45a3c4c44fb66e9ab20502f7085aca2d593e673687b4347b6954a28`, sync `8e6f31878360a88ccc3ef542c6ac825ea725afd4b0775b1d6d91fea4ecabc015`.
- Device IDs are unchanged.
- New server-account passwords and new permanent passwords must be at least 8 characters. Existing passwords still verify.
- Login tokens on disk are hashed the next time the server starts. Clients keep sending the raw token.
- Automatic update from 0.3.7 onward requires `SHA256SUMS` and a GitHub download URL.
- Signup no longer returns HTTP 409 or a `sync_enabled` field. The website signs in after signup and does not depend on those.
- Data-folder and service-folder permissions tighten the next time each service starts.

## Checks run

On Linux, with Rust 1.99:

- `cargo test -p pjdesk-proto -p pjdesk-server` — 17 proto tests and 21 server tests passed, including the account-key test vector, the AWS SigV4 example, token hashing, and the checksum and download-URL checks.
- `cargo check -p pjdesk --target x86_64-pc-windows-gnu` — passed (the client's own tests are not linked here; they need the Windows C++ runtime).
- `node --check` on the Worker and the site scripts — passed.
- `npm audit` in `cloud/` — no vulnerabilities.
- `cargo audit` 0.22.2 — no vulnerabilities. Four warnings, described under A8, left as-is.

The installers were not built. The Worker was not deployed.
