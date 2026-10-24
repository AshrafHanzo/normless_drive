-- Let a share link point at specific files rather than a whole folder.
--
-- NULL means "everything in the folder", which is what every existing
-- link means and must keep meaning. A non-empty array restricts the link
-- to exactly those messages — so you can hand someone three files out of
-- a folder of three hundred without moving anything.

ALTER TABLE shares ADD COLUMN IF NOT EXISTS message_ids INTEGER[];

COMMENT ON COLUMN shares.message_ids IS
  'Telegram message ids this link exposes. NULL exposes the whole folder.';
