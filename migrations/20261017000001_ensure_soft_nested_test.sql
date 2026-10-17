-- Fixtures for `create_or_find_in_op` on a soft-delete repo with a nested child.
CREATE TABLE ensure_soft_boxes (
  id UUID PRIMARY KEY,
  label VARCHAR NOT NULL UNIQUE,
  deleted BOOL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE ensure_soft_box_events (
  id UUID NOT NULL REFERENCES ensure_soft_boxes(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
CREATE TABLE ensure_soft_notes (
  id UUID PRIMARY KEY,
  box_id UUID NOT NULL REFERENCES ensure_soft_boxes(id),
  deleted BOOL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE ensure_soft_note_events (
  id UUID NOT NULL REFERENCES ensure_soft_notes(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
