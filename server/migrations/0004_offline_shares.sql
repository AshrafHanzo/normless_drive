-- Lets a share link keep working when its owner is signed out.
--
-- The problem this solves: a Telegram session is encrypted with a key
-- derived from the owner's password, which exists only while they are
-- signed in. A public share link opened by a stranger at 3am has no such
-- key, so without this a link works only while the owner has a browser tab
-- open — the same limitation as the desktop app needing to be running,
-- which is the entire reason this web app exists.
--
-- The only way a server can act for an absent user is to hold a key that
-- does not depend on that user being present. So a *second* copy of the
-- session is stored, wrapped with a key derived from a secret held in the
-- server's environment (SHARE_SECRET), not in the database.
--
-- What that buys and what it costs, stated plainly:
--
--   * A stolen database alone is still useless: the secret is not in it.
--   * Someone with both the database and the server's environment can
--     decrypt the sessions of every user who enabled this. The per-password
--     copy does not have that weakness.
--   * It is therefore OPT-IN per user. Nothing is written here unless
--     someone deliberately turns on offline sharing.

ALTER TABLE tg_sessions ADD COLUMN IF NOT EXISTS session_srv_enc   BYTEA;
ALTER TABLE tg_sessions ADD COLUMN IF NOT EXISTS session_srv_nonce BYTEA;
ALTER TABLE tg_sessions ADD COLUMN IF NOT EXISTS api_hash_srv_enc   BYTEA;
ALTER TABLE tg_sessions ADD COLUMN IF NOT EXISTS api_hash_srv_nonce BYTEA;

-- When this was turned on, so the UI can say so and an audit can show it.
ALTER TABLE tg_sessions ADD COLUMN IF NOT EXISTS offline_enabled_at TIMESTAMPTZ;

COMMENT ON COLUMN tg_sessions.session_srv_enc IS
  'Session wrapped with the server secret so share links work while the '
  'owner is signed out. NULL unless the owner opted in.';

-- Shares need to know whether they may use that copy. A share created
-- before the owner opted in should not silently start working offline.
ALTER TABLE shares ADD COLUMN IF NOT EXISTS allow_offline BOOLEAN NOT NULL DEFAULT FALSE;
