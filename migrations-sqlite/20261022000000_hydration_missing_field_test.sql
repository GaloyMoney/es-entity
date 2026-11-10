-- Fixtures for tests/hydration_missing_field.rs: rows are planted directly so
-- the event streams can omit fields the entity requires.
CREATE TABLE hydration_probes (
  id BLOB PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE hydration_probe_events (
  id BLOB NOT NULL REFERENCES hydration_probes(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
