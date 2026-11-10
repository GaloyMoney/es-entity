-- Fixtures for `create_or_find_in_op` on soft-delete repos. `email` is a full
-- unique key a deleted row keeps; `handle` is unique among live rows only.
CREATE TABLE ensure_soft_members (
  id BLOB PRIMARY KEY NOT NULL,
  email TEXT NOT NULL UNIQUE,
  handle TEXT NOT NULL,
  deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX ensure_soft_members_live_handle_idx ON ensure_soft_members (handle) WHERE deleted = FALSE;
CREATE TABLE ensure_soft_member_events (
  id BLOB NOT NULL REFERENCES ensure_soft_members(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
