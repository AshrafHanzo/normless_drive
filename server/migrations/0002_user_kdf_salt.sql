-- Move the key-derivation salt onto `users`.
--
-- It lived on `tg_sessions`, which only exists once someone has connected
-- Telegram. But the wrapping key has to be derived at *login* — that is the
-- only moment the plaintext password passes through the server — and a user
-- logs in long before they connect anything. With the salt on tg_sessions
-- there was nothing to derive from at that point, so the key could never be
-- cached and the session could never be decrypted for an ordinary request.

ALTER TABLE users ADD COLUMN IF NOT EXISTS kdf_salt BYTEA;

-- Backfill. Safe to generate randomly: a salt is only meaningful once
-- something has been encrypted under the key it produces, and any row
-- needing a backfill by definition has no tg_sessions row yet.
UPDATE users
   SET kdf_salt = gen_random_bytes(16)
 WHERE kdf_salt IS NULL;

ALTER TABLE users ALTER COLUMN kdf_salt SET NOT NULL;

-- tg_sessions.kdf_salt is now redundant. Left in place rather than dropped:
-- nothing reads it, dropping a column is the one migration that cannot be
-- rolled back by re-running, and it costs a few bytes a row.
COMMENT ON COLUMN tg_sessions.kdf_salt IS
  'Unused since 0002. The live salt is users.kdf_salt.';
