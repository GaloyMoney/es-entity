-- Fixtures for tests/find_all_for_opt_in.rs: entries keep their list_for
-- columns inside a values struct instead of as entity fields.
CREATE TABLE opt_in_entries (
  id BLOB PRIMARY KEY NOT NULL,
  account_id BLOB NOT NULL,
  visibility TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_opt_in_entries_account_created_id ON opt_in_entries (account_id, created_at DESC, id DESC);
CREATE INDEX idx_opt_in_entries_visibility_created_id ON opt_in_entries (visibility, created_at DESC, id DESC);

CREATE TABLE opt_in_entry_events (
  id BLOB NOT NULL REFERENCES opt_in_entries(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
