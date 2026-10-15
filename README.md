# Normless Drive

Browser front end for Telegram-backed file storage, with its own accounts and
an administrator who controls who may sign in. Deployed at
`drive.normless.in`.

Derived from the Telegram Drive desktop app; the architecture, what is reused
and the tradeoffs chosen are written down in
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md). Read section 2 before touching
anything that handles a Telegram session.

## Status

Early. What exists today:

- [x] Schema — users, Telegram sessions, folders, shares, app sessions, audit
- [x] Accounts — register, log in, log out, `/api/auth/me`
- [x] Session cookies — random token, only its SHA-256 stored
- [x] Session-at-rest encryption primitives (Argon2id + XChaCha20-Poly1305)
- [x] Admin — list users, approve/suspend, grant/revoke admin, read the audit log
- [ ] Telegram connect flow (phone → code → encrypted session)
- [ ] File browse / upload / download
- [ ] Share links
- [ ] Web UI

## Access model

The service is **closed by default**. Registering creates a `pending`
account that cannot sign in until an administrator approves it.

The **first account to register becomes the administrator** and is active
immediately — otherwise a fresh deployment would have nobody able to approve
anyone, including themselves. Register your own account first, before
anybody else does.

## Running locally

Needs Rust stable and a PostgreSQL 16 database.

```bash
cd server
cp .env.example .env     # then edit DATABASE_URL
cargo run
```

| Variable | Required | Default | Meaning |
| --- | --- | --- | --- |
| `DATABASE_URL` | yes | — | `postgres://user:pass@host/db` |
| `PUBLIC_BASE_URL` | yes | — | Origin share links are built from |
| `BIND_ADDR` | no | `127.0.0.1:8080` | Listen address |
| `SESSION_TTL_HOURS` | no | `336` (14 days) | Login session lifetime |
| `COOKIE_SECURE` | no | `true` | Set `false` only for local plain HTTP |

The schema is applied automatically at startup.

```bash
cargo test        # 23 tests, no database required
cargo check       # type check
```

## API so far

| Method | Path | Auth | Purpose |
| --- | --- | --- | --- |
| `GET` | `/health` | none | Liveness |
| `POST` | `/api/auth/register` | none | Create a pending account |
| `POST` | `/api/auth/login` | none | Sign in, sets `nd_session` cookie |
| `POST` | `/api/auth/logout` | cookie | Delete the session |
| `GET` | `/api/auth/me` | cookie | Current account |
| `GET` | `/api/admin/users` | admin | List users, pending first |
| `POST` | `/api/admin/users/{id}/status` | admin | `pending` / `active` / `suspended` |
| `POST` | `/api/admin/users/{id}/role` | admin | `admin` / `user` |
| `GET` | `/api/admin/audit` | admin | Recent audit entries |

## Deployment

See [deploy/nginx-drive.normless.in.conf](deploy/nginx-drive.normless.in.conf).
Three settings in it are load-bearing and easy to get wrong:
`client_max_body_size 0`, `proxy_request_buffering off`, and the one-hour
timeouts. Without them large uploads fail at the proxy before the
application ever sees them.
