-- Fixture for an explicitly-named primary key: proves the pkey's actual
-- catalog name (not the `{table}_pkey` convention) threads through both the
-- generated ConstraintViolation enum and the create-path classifier.
CREATE TABLE v2_widgets (
  id BLOB NOT NULL,
  name TEXT NOT NULL,
  created_at TEXT NOT NULL,
  CONSTRAINT v2_widgets_id_pk PRIMARY KEY (id)
);
CREATE TABLE v2_widget_events (
  id BLOB NOT NULL REFERENCES v2_widgets(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
