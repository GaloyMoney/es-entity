-- Fixture for an explicitly-named primary key: proves the pkey's actual
-- catalog name (not the `{table}_pkey` convention) threads through both the
-- generated ConstraintViolation enum and the create-path classifier.
CREATE TABLE v2_widgets (
  id UUID NOT NULL,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  CONSTRAINT v2_widgets_id_pk PRIMARY KEY (id)
);
CREATE TABLE v2_widget_events (
  id UUID NOT NULL REFERENCES v2_widgets(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
