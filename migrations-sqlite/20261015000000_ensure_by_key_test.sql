-- Fixtures for `create_or_find_by_<key>_in_op`. `ensure_templates` has no
-- constraint besides its primary key and one unique column, `ensure_slots` a
-- composite and a single-column unique key.
CREATE TABLE ensure_templates (
  id BLOB PRIMARY KEY NOT NULL,
  code TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);
CREATE TABLE ensure_template_events (
  id BLOB NOT NULL REFERENCES ensure_templates(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ensure_slots (
  id BLOB PRIMARY KEY NOT NULL,
  account TEXT NOT NULL,
  currency TEXT NOT NULL,
  label TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL,
  UNIQUE (account, currency)
);
CREATE TABLE ensure_slot_events (
  id BLOB NOT NULL REFERENCES ensure_slots(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
