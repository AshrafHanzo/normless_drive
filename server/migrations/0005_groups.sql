-- Folder groups, matching the desktop app's sidebar concept.
--
-- A group is purely an organisational label: it holds folders, never
-- files, and exists only in this database. Nothing about it reaches
-- Telegram, which is why it cannot sync between devices the way folders
-- do — a folder is a real Telegram channel, a group is a sticker on it.

CREATE TABLE IF NOT EXISTS groups (
    id            UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name          TEXT NOT NULL,
    color_hex     TEXT NOT NULL DEFAULT '#2d7ff9',
    display_order INTEGER NOT NULL DEFAULT 0,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS groups_user_idx ON groups (user_id, display_order);

-- ON DELETE SET NULL, never CASCADE: deleting a group must not delete the
-- folders in it, and a folder is a Telegram channel full of real files.
-- Getting this wrong once would be unrecoverable.
ALTER TABLE folders
    ADD COLUMN IF NOT EXISTS group_id UUID REFERENCES groups(id) ON DELETE SET NULL;

CREATE INDEX IF NOT EXISTS folders_group_idx ON folders (group_id);
