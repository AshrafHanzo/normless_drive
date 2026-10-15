-- Normless Drive — initial schema.
--
-- Every table that can hold user data carries user_id and is only ever
-- queried with it in the WHERE clause. This is a multi-tenant service built
-- from a single-user codebase, so the whole class of bug worth fearing here
-- is one tenant reading another's rows.

CREATE TABLE IF NOT EXISTS users (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email           TEXT NOT NULL,
    -- Argon2id PHC string. Carries its own salt and parameters, so there is
    -- no separate salt column and the cost can be raised later without a
    -- migration — existing hashes keep verifying against their own params.
    password_hash   TEXT NOT NULL,
    role            TEXT NOT NULL DEFAULT 'user',
    -- New signups land on 'pending' and cannot log in until an admin moves
    -- them to 'active'. The service is closed by default: registration being
    -- open is not the same as access being open.
    status          TEXT NOT NULL DEFAULT 'pending',
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at   TIMESTAMPTZ,

    CONSTRAINT users_role_valid   CHECK (role   IN ('admin', 'user')),
    CONSTRAINT users_status_valid CHECK (status IN ('pending', 'active', 'suspended'))
);

-- Case-insensitive uniqueness. Storing the address as typed but matching
-- folded, so "Ash@x.com" cannot register alongside "ash@x.com".
CREATE UNIQUE INDEX IF NOT EXISTS users_email_lower_key ON users (lower(email));


-- A user's Telegram connection. At most one per user.
--
-- session_enc is XChaCha20-Poly1305 over the MTProto session, keyed by a
-- value derived from that user's password (see docs/ARCHITECTURE.md 2.1).
-- The server cannot read any row here without the user being logged in, by
-- design: a stolen dump of this table is inert.
CREATE TABLE IF NOT EXISTS tg_sessions (
    user_id         UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    api_id          INTEGER NOT NULL,
    api_hash_enc    BYTEA NOT NULL,
    api_hash_nonce  BYTEA NOT NULL,
    session_enc     BYTEA NOT NULL,
    session_nonce   BYTEA NOT NULL,
    -- Argon2id salt for deriving the wrapping key from the user's password.
    -- Distinct from the salt inside users.password_hash: reusing that one
    -- would make the login hash and the encryption key relatives.
    kdf_salt        BYTEA NOT NULL,
    phone_hint      TEXT,
    connected_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);


-- Telegram channels acting as folders, per user.
CREATE TABLE IF NOT EXISTS folders (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    channel_id      BIGINT NOT NULL,
    name            TEXT NOT NULL,
    display_order   INTEGER NOT NULL DEFAULT 0,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),

    -- The same channel must not be registered twice for one user, but two
    -- users may legitimately reference different channels with equal ids
    -- only by coincidence — so the constraint is on the pair, not channel_id.
    CONSTRAINT folders_user_channel_key UNIQUE (user_id, channel_id)
);

CREATE INDEX IF NOT EXISTS folders_user_idx ON folders (user_id, display_order);


-- Public share links. Permissions are the same bitfield the desktop app
-- uses: 1 UPLOAD, 2 DOWNLOAD, 4 UPDATE, 8 DELETE.
CREATE TABLE IF NOT EXISTS shares (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    folder_id       UUID REFERENCES folders(id) ON DELETE CASCADE,
    token           TEXT NOT NULL UNIQUE,
    permissions     BIGINT NOT NULL DEFAULT 2,
    -- bcrypt/Argon2 hash, never the password itself. NULL means no password.
    password_hash   TEXT,
    expires_at      TIMESTAMPTZ,
    usage_limit     INTEGER,
    usage_count     INTEGER NOT NULL DEFAULT 0,
    revoked         BOOLEAN NOT NULL DEFAULT FALSE,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Share resolution happens on every request to /s/{token}; it must not be a
-- sequential scan once there are a few thousand links.
CREATE INDEX IF NOT EXISTS shares_token_idx ON shares (token) WHERE NOT revoked;
CREATE INDEX IF NOT EXISTS shares_user_idx  ON shares (user_id, created_at DESC);


-- Login sessions for the web app itself (not Telegram sessions).
CREATE TABLE IF NOT EXISTS app_sessions (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    -- SHA-256 of the cookie value. A dump of this table must not let anyone
    -- resume a live session, so the raw token is never stored.
    token_hash      BYTEA NOT NULL UNIQUE,
    expires_at      TIMESTAMPTZ NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    user_agent      TEXT,
    ip              INET
);

CREATE INDEX IF NOT EXISTS app_sessions_user_idx    ON app_sessions (user_id);
CREATE INDEX IF NOT EXISTS app_sessions_expiry_idx  ON app_sessions (expires_at);


-- Append-only record of things worth being able to reconstruct later:
-- who was approved, who connected Telegram, who shared what.
CREATE TABLE IF NOT EXISTS audit (
    id              BIGSERIAL PRIMARY KEY,
    user_id         UUID REFERENCES users(id) ON DELETE SET NULL,
    actor_id        UUID REFERENCES users(id) ON DELETE SET NULL,
    action          TEXT NOT NULL,
    detail          JSONB,
    ip              INET,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS audit_user_idx    ON audit (user_id, created_at DESC);
CREATE INDEX IF NOT EXISTS audit_created_idx ON audit (created_at DESC);
