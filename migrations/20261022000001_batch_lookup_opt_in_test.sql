-- Fixtures for tests/find_all_for_opt_in.rs: entries keep their list_for
-- columns inside a values struct instead of as entity fields.
CREATE TABLE opt_in_entries (
  id UUID PRIMARY KEY,
  account_id UUID NOT NULL,
  visibility VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_opt_in_entries_account_created_id ON opt_in_entries (account_id, created_at DESC, id DESC);
CREATE INDEX idx_opt_in_entries_visibility_created_id ON opt_in_entries (visibility, created_at DESC, id DESC);

CREATE TABLE opt_in_entry_events (
  id UUID NOT NULL REFERENCES opt_in_entries(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
