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
- **Downloads**: `/download/client|server|sums` redirect to the latest GitHub
  release's files, so the site never needs editing per release.

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
installs Cloudflare's deploy tool on first run, signs in to Cloudflare as
pjdesk@spader.us (browser, first time only), applies database migrations and
deploys. `wrangler.toml` holds the account, D1 database ID and bucket name.
