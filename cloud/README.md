# PJDesk Cloud

The pjdesk.com website plus the PJDesk Cloud service: PJDesk accounts and the
encrypted server copies behind **Cloud Sync** (a backup PJDesk Server that
takes over when you point your server's DNS name at it).

One Cloudflare Worker (`src/index.js`) with:

- **Static assets** (`public/`): the website, `/account/` (sign up, sign in,
  change password, servers signed in) and `/admin/` (turn Cloud Sync on per
  account).
- **D1** (`DB`): accounts, sessions, sync bookkeeping (`migrations/`).
- **R2** (`SYNC`, bucket `pjdesk-sync`): one sealed copy per account.
- **R2** (`RELEASES`, bucket `pjdesk-releases`): installers only. Encrypted
  account copies stay in `pjdesk-sync`. Do not reuse that bucket for releases.
- **Downloads**: `/dl/latest/<file>` and `/dl/<version>/<file>` serve those
  installers from R2. `/download/client|server|sums` redirect there once a
  release is published (and still fall back to GitHub if R2 is empty). Opening
  a file needs the download-password cookie, a signed-in account Bearer token,
  `Authorization: Updater` (`UPDATE_TOKEN`), `Authorization: Admin`
  (`RELEASE_ADMIN_KEY`), or a short-lived `?t=` ticket. Browsers without
  access are sent to `/#download`; other callers get `401` with
  `{code:"download_password"}`.
- **Updates**: authenticated `GET /api/release` returns pjdesk.com `/dl/` URLs
  with tickets. While `RELEASE_PUBLIC_FALLBACK` is `"1"` (the default),
  unauthenticated callers still get GitHub URLs so 0.6.90 and older can
  update. Set it to `"0"` only after those installs are on a build that
  fetches `/dl/` with auth.

## Privacy

Passwords never reach the service. Browsers and servers stretch the password
with PBKDF2-SHA256 (600,000 rounds, salted with the email) and derive two keys:
an *auth key* that is sent to sign in (stored as a salted PBKDF2 hash), and a
*sync key* that seals the server copy with ChaCha20-Poly1305 and never leaves
the customer's servers. PJDesk Cloud only ever holds ciphertext, so it can't
read a customer's server key, accounts or address books, and it can't recover
a forgotten password.

## Accounts and Cloud Sync

New accounts start on the Free plan (Cloud Sync off). An administrator grants
Cloud Sync at `/admin/` — optional expiry, a note, and an audit row for every
change. Accounts listed in `ADMIN_EMAILS` are administrators and always have
Cloud Sync. The site does not list prices, so Free and Cloud Sync are the only
plans. A grant that reaches its expiry turns Cloud Sync off the next time that
account is used. `deploy.cmd` applies D1 migrations, including `0002_plans.sql`,
before it deploys the Worker.

## Download password

The download section on pjdesk.com asks for a password, the same way ngnvr.com
does: the Worker checks it, it is never in the source, and a right guess gives
that browser an HttpOnly cookie for 30 days. Ten wrong guesses from one
address are locked out for 15 minutes. Changing the secret ends every such
cookie. If the secret is not set, the website shows "Downloads aren't open yet."

Set the secret from your PC (Wrangler prompts for the password; it is not
echoed). End with a blank line:

```
cd "C:\App Dev\PJDesk\cloud"
npx.cmd wrangler secret put DOWNLOAD_PASSWORD
```

### Locked files (R2)

Installers live in the `pjdesk-releases` bucket (Worker binding `RELEASES`),
not in `pjdesk-sync`. Create that bucket yourself before the first deploy that
includes the binding; this repo does not create it.

A browser that already unlocked the download form can fetch `/dl/...` with the
cookie. A signed-in pjdesk.com account can send `Authorization: Bearer`. The
Windows and Android updaters in 0.6.91+ send that account token when the
machine is signed in to Cloud Sync or a website account, or `Authorization:
Updater` when `PJDESK_UPDATE_TOKEN` is set at runtime or baked in at build
time with the same-named compile-time environment variable. A token compiled
into a public binary is extractable; it is still stronger than a User-Agent
string, and it is never committed in source. Android cannot send custom
headers when it opens a download URL, so `/api/release` includes a one-hour
signed `?t=` ticket on each file URL. The publish script uses
`Authorization: Admin` (`RELEASE_ADMIN_KEY`).

0.6.90 and older do not send any of those credentials. They either call
`GET /api/release` with only `User-Agent: PJDesk-Updater` (Windows Server,
since 0.4.0) or talk only to the GitHub API (Windows Client and Android;
also every server before 0.4.0). From 0.3.7 on, the Windows updater
refuses any download URL whose host is not GitHub
(`github.com` / `*.githubusercontent.com`) before it even issues the GET.
SHA-256 against `SHA256SUMS` is required on Windows, and that sums URL is
checked the same way. ureq would follow redirects, but that never helps:
the host check is on the URL in the feed, not the final hop, and a private
GitHub repo will not serve a redirect we control. A short-lived
`?t=` ticket on a pjdesk.com URL also breaks 0.6.90 server name parsing
(`rsplit('/')` leaves `file.exe?t=...`, which does not end in `.exe`), so
it would fall back to the GitHub API anyway.

So while `RELEASE_PUBLIC_FALLBACK` is `"1"`, unauthenticated `/api/release`
keeps returning GitHub URLs. Do not point those old installs at `/dl/` —
they would refuse the URL or fail the name check. A failed update leaves
the machine running. After the GitHub repository is private, those old
installs cannot be kept updating. Only 0.6.91 and later, which trust
pjdesk.com `/dl/` and send auth (or use the ticket already on the URL),
will still auto-update.

For 0.6.91+ to keep updating after the repo is private even when the
machine is not signed in to a pjdesk.com account, bake
`PJDESK_UPDATE_TOKEN` into that build (and set the same value as the
Worker secret `UPDATE_TOKEN`). A token compiled into the binary is
extractable; it is still stronger than a User-Agent string, and it must
not be committed in source.

Upload a release from the folder that holds the six files (client setup,
setup, server setup, APK, SHA256SUMS, cloud zip):

```
cd "C:\App Dev\PJDesk\cloud"
$env:RELEASE_ADMIN_KEY = "..."   # the Worker secret, not committed
npx.cmd node tools/publish-release.mjs --dir "C:\path\to\release-0.6.91" --version 0.6.91
```

The files are not truly locked while the same installers remain public GitHub
release assets on `whoamib2/pjdesk`. Making that repository private (or
removing the assets) ends public downloads, and it also ends auto-update
for every install still on 0.6.90 or older. It will also hide the source
from people who cloned the public repo. Do that only after the machines
you care about are already on 0.6.91 or later.

Until then, publish each new version to both R2 and GitHub, and leave
`RELEASE_PUBLIC_FALLBACK` at `"1"` so 0.6.90 servers can still reach 0.6.91
from the public assets. After the repo is private, set
`RELEASE_PUBLIC_FALLBACK` to `"0"`: unauthenticated `/api/release` still
names the current version (so a current install sees it is up to date) but
returns empty file URLs instead of GitHub links that would 404. An older
server then falls back to GitHub, fails, and stays on the old version,
still online.

None of bucket creation, secret changes, deploys, DNS, or GitHub asset
removal is done from this tree. Those are decisions for the person who
publishes.

## Develop

```
cd cloud
npm install
npm run migrate:local
npm run dev            # http://127.0.0.1:8787
```

Point a server at it with `PJDESK_CLOUD_API=http://127.0.0.1:8787` (window) or
`pjdesk-server cloud signin --api http://127.0.0.1:8787 --email ...`.

## Deploy

From your PC (no CI): open a terminal in `cloud\` and run `deploy.cmd`. It
installs Cloudflare's deploy tool on first run, checks that Wrangler is signed
in as pjdesk@spader.us, applies database migrations, and deploys. If Wrangler
is not signed in, or a later step fails, it stops and exits with an error. It
does not print "PJDesk Cloud is deployed." after a failure. Sign in with
`npx wrangler login`. `wrangler.toml` holds the account, D1 database ID and
bucket name.
