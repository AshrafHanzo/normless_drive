# Normless Drive — architecture

A browser-based Telegram storage front end, deployed at `drive.normless.in`,
with its own user accounts and an admin who controls who may sign in.

Derived from the Telegram Drive desktop app (Tauri + Rust + React). This
document records what is being built, what is being reused, and which
tradeoffs were chosen deliberately.

---

## 1. Why a web app at all

The desktop app already does the file work well. Three things it cannot do:

| Problem | Desktop app | Web app |
| --- | --- | --- |
| Share links die when the PC sleeps | tunnel is a child process of the app | server is always up |
| macOS / Linux users | needs a signed build per platform | any browser |
| Deciding who may use it | no concept of users | accounts + admin approval |

Those three are the entire reason this project exists. Anything that does not
serve them is out of scope for v1.

---

## 2. The storage model, and what it costs

**Each user connects their own Telegram account.** Their files live in their
own Telegram; this server never becomes the owner of anyone's data.

The price of that choice is specific and worth stating plainly, because the
whole security design below exists to blunt it:

> To act on a user's Telegram account, this server must hold that user's
> MTProto **session**. A session is not a scoped token. It is equivalent to
> being logged in as them: read every message, send messages as them, read
> their contact list. There is no narrower credential Telegram offers.

So a full compromise of this VPS is a full compromise of every connected
user's Telegram account, not merely of the files they stored here.

Mitigations, in order of how much they actually buy:

### 2.1 Sessions are encrypted with a key derived from the user's password

A session is **never** written to disk in usable form.

```
K        = Argon2id(user_password, per_user_salt)
stored   = XChaCha20-Poly1305(K, session_string)
```

`K` exists only in server memory, held in a cache keyed by the user's live
login, and is dropped when they log out or the entry expires. The database
stores only `stored` and the salt.

**What this buys:** a stolen database dump — the most common real-world breach
— yields nothing. An attacker needs the ciphertext *and* the user's password
*and* must act while that user is logged in.

**What it does not buy:** an attacker with live code execution on the box can
read `K` out of memory for every currently-logged-in user. This design defends
against data-at-rest theft, not against root on a running server.

**The tradeoff it forces:** no background work for a logged-out user. Nothing
can run a scheduled backup at 3am for someone who signed out, because the key
to their session is gone until they sign in again. This is accepted. The
alternative — a server-held master key — would mean the database dump alone is
enough, which defeats the point.

### 2.2 Each user brings their own api_id / api_hash

Telegram rate-limits and bans per API credential. One shared credential across
all users means one user's bulk upload gets *everyone* throttled, and one
abuse report kills the whole service. Per-user credentials contain the blast
radius, and make each connection honestly what it is: that user's own client.

### 2.3 Informed consent, not a surprise

Before the phone-number field is ever shown, the user sees a plain-language
screen stating that the server will hold a credential equivalent to full
access to their Telegram account, and what that means. Disconnecting is one
click and deletes the stored session immediately.

This is not legal cover. It is so that nobody can say they did not know.

---

## 3. Bandwidth: every byte crosses this server

A browser cannot speak MTProto. So uploads are
`browser → VPS → Telegram`, and downloads are the reverse.

**A 10 GB upload costs 20 GB of VPS transfer.** The desktop app has no such
cost because each user's own machine does the work. Check the provider's
monthly allowance before onboarding users; this is the limit most likely to be
hit first, well before disk or CPU.

Nothing is buffered to disk: bodies stream straight through
(`tokio_util::io::StreamReader` into grammers' `upload_stream`), the same way
`folder_share_routes::upload_file_streaming` already does in the desktop app.

---

## 4. Stack, and why

| Layer | Choice | Reason |
| --- | --- | --- |
| Backend | **Rust + actix-web** | The desktop app's HTTP layer is already actix. `api_routes.rs` (1,791 lines) and `folder_share_routes.rs` (2,507 lines) are real handlers with chunked upload, HTTP Range, and share permissions already working. |
| Telegram | **grammers** (git rev `d07f96f`) | Same pin as the desktop app. Full MTProto, no 20 MB Bot API cap. |
| Frontend | **React 19 + TS + Tailwind 4** | Ported from the desktop app's `app/src` — 21,512 lines that already run in a browser engine. |
| Database | **PostgreSQL 16** | Already running on the VPS for the CRM. Separate database, separate role. |
| Process | **systemd** behind **nginx** | Matches how the CRM is already deployed. |

Rust over Node is not a preference. Rewriting in Node means reimplementing the
2 GB splitter, the manifest format, chunked resumable upload, and Range
handling — all of which exist and are tested here.

---

## 5. What ports over, and how hard

| Piece | Source | Effort |
| --- | --- | --- |
| File browser, viewers, transfer queue, themes, i18n | `app/src` | **Mechanical.** 205 `invoke()` calls across 29 files become `fetch()`. |
| Telegram client, 2 GB splitter, SHA-256 manifest | `split_file.rs`, `commands/fs.rs` | **Light.** Strip Tauri state, pass a per-request client. |
| Share links + permissions | `folder_share_routes.rs` | **Light.** Already actix. Needs an owner column. |
| Tauri plugins (dialog, store, shell, fs, updater) | 65 imports | **Replace.** Browser APIs: file input, localStorage, download attr. |
| Auth, accounts, admin, multi-tenancy | — | **New.** Does not exist in the desktop app at all. |

The desktop app is single-user throughout. Every table and route gains an
owner, and every query gains a `WHERE user_id = ?`. That is the bulk of the
genuinely new work, and the easiest place to introduce a data-leak bug.

---

## 6. Data model (first cut)

```
users          id, email, password_hash (Argon2id), role, status, created_at
tg_sessions    user_id, api_id, api_hash_enc, session_enc, salt, nonce, connected_at
folders        id, user_id, channel_id, name, display_order
shares         id, user_id, folder_id, token, permissions, password_hash,
               expires_at, usage_limit, usage_count, revoked
audit          id, user_id, action, detail, ip, created_at
```

`role` is `admin | user`. `status` is `pending | active | suspended` — new
signups land on `pending` and an admin promotes them, so the service is not
open to the public by default.

---

## 7. Deployment

```
drive.normless.in  --A-->  163.128.112.31
                             nginx :443  (certbot)
                               proxy_pass -> 127.0.0.1:8080
                                 normless-drive.service (systemd)
                                   PostgreSQL :5432  (loopback)
```

nginx needs `client_max_body_size 0` and long `proxy_read_timeout` on the
upload route, or large uploads die at the proxy before reaching the app.

Runs as its own unprivileged user, not root, and not alongside the CRM's
process. A bug here must not be able to reach the CRM's database.

---

## 8. Explicitly out of scope for v1

- Encryption-at-rest of file contents (TDENC2). Separate feature, large.
- Video transcoding / HLS. CPU-expensive and shared across tenants.
- WebDAV.
- Mobile apps.
- Background scheduled backups — impossible for logged-out users under the
  key design in §2.1. Revisit only with a deliberate decision about §2.1.
