-- Fixtures for `create_or_find_in_op` on soft-delete repos. `email` is a full
-- unique key a deleted row keeps; `handle` is unique among live rows only.
CREATE TABLE ensure_soft_members (
  id UUID PRIMARY KEY,
  email VARCHAR NOT NULL UNIQUE,
  handle VARCHAR NOT NULL,
  deleted BOOL NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE UNIQUE INDEX ensure_soft_members_live_handle_idx ON ensure_soft_members (handle) WHERE deleted = FALSE;
CREATE TABLE ensure_soft_member_events (
  id UUID NOT NULL REFERENCES ensure_soft_members(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
