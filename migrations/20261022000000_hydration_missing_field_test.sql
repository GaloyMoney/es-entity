-- Fixtures for tests/hydration_missing_field.rs: rows are planted directly so
-- the event streams can omit fields the entity requires.
CREATE TABLE hydration_probes (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE hydration_probe_events (
  id UUID NOT NULL REFERENCES hydration_probes(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
