-- Fixtures for `create_or_find_by_<key>_in_op`. `ensure_templates` has no
-- constraint besides its primary key and one unique column, `ensure_slots` a
-- composite and a single-column unique key.
CREATE TABLE ensure_templates (
  id UUID PRIMARY KEY,
  code VARCHAR NOT NULL UNIQUE,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE ensure_template_events (
  id UUID NOT NULL REFERENCES ensure_templates(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ensure_slots (
  id UUID PRIMARY KEY,
  account VARCHAR NOT NULL,
  currency VARCHAR NOT NULL,
  label VARCHAR NOT NULL UNIQUE,
  created_at TIMESTAMPTZ NOT NULL,
  UNIQUE (account, currency)
);
CREATE TABLE ensure_slot_events (
  id UUID NOT NULL REFERENCES ensure_slots(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
