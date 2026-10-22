-- Folders, share links, and a repair for mangled phone hints.

-- 1. Repair phone_hint values written before the hint became ASCII.
--
-- An earlier build stored a bullet-prefixed hint, and a PowerShell edit
-- double-encoded it on the way into the source, so rows written then read
-- as mojibake. Rewriting them here means nobody has to disconnect and
-- reconnect to clear it.
UPDATE tg_sessions
   SET phone_hint = '...' || right(regexp_replace(phone_hint, '[^0-9]', '', 'g'), 3)
 WHERE phone_hint IS NOT NULL
   AND phone_hint !~ '^\.\.\.[0-9]{1,3}$';


-- 2. Folders are Telegram channels. `channel_id` is Telegram's own id;
-- `access_hash` is required to address a channel and is issued per account,
-- so it is stored alongside rather than re-resolved on every request.
ALTER TABLE folders ADD COLUMN IF NOT EXISTS access_hash BIGINT;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS is_public BOOLEAN NOT NULL DEFAULT FALSE;
ALTER TABLE folders ADD COLUMN IF NOT EXISTS username TEXT;


-- 3. Share links.
--
-- `folder_id` NULL means the share points at Saved Messages, which has no
-- folders row of its own.
ALTER TABLE shares ADD COLUMN IF NOT EXISTS label TEXT;
ALTER TABLE shares ADD COLUMN IF NOT EXISTS last_used_at TIMESTAMPTZ;

-- Share tokens are looked up on every request to a public link, including
-- by people who are not signed in, so this index is on the hot path.
CREATE INDEX IF NOT EXISTS shares_token_live_idx
    ON shares (token)
 WHERE NOT revoked;


-- 4. Per-user feature permissions.
--
-- An administrator decides what each account may do. Defaults are
-- deliberately permissive for existing accounts (they could already do
-- these things) but the admin screen can withdraw any of them.
CREATE TABLE IF NOT EXISTS user_permissions (
    user_id         UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    can_upload      BOOLEAN NOT NULL DEFAULT TRUE,
    can_download    BOOLEAN NOT NULL DEFAULT TRUE,
    can_delete      BOOLEAN NOT NULL DEFAULT TRUE,
    can_create_folders BOOLEAN NOT NULL DEFAULT TRUE,
    can_share       BOOLEAN NOT NULL DEFAULT TRUE,
    -- 0 means no cap. Bytes, counted against this user's own uploads.
    storage_quota_bytes BIGINT NOT NULL DEFAULT 0,
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Every existing account gets a row, so the rest of the code can assume
-- one exists rather than scattering COALESCE over every permission check.
INSERT INTO user_permissions (user_id)
SELECT id FROM users
    ON CONFLICT (user_id) DO NOTHING;


-- 5. A record of what each account has uploaded.
--
-- Needed to enforce a storage quota. Telegram itself is the only authority
-- on what actually exists, but asking it to total a user's files would mean
-- walking every message on every upload — so this is a local tally,
-- deliberately an approximation: a file deleted directly in the Telegram
-- app will not be noticed here.
CREATE TABLE IF NOT EXISTS uploads (
    id          BIGSERIAL PRIMARY KEY,
    user_id     UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    folder_id   UUID REFERENCES folders(id) ON DELETE SET NULL,
    message_id  INTEGER NOT NULL,
    name        TEXT NOT NULL,
    size_bytes  BIGINT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS uploads_user_idx ON uploads (user_id, created_at DESC);
