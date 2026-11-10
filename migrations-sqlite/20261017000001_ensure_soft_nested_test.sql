-- Fixtures for `create_or_find_in_op` on a soft-delete repo with a nested child.
CREATE TABLE ensure_soft_boxes (
  id BLOB PRIMARY KEY NOT NULL,
  label TEXT NOT NULL UNIQUE,
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);
CREATE TABLE ensure_soft_box_events (
  id BLOB NOT NULL REFERENCES ensure_soft_boxes(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
CREATE TABLE ensure_soft_notes (
  id BLOB PRIMARY KEY NOT NULL,
  box_id BLOB NOT NULL REFERENCES ensure_soft_boxes(id),
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);
CREATE TABLE ensure_soft_note_events (
  id BLOB NOT NULL REFERENCES ensure_soft_notes(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
