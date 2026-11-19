-- Fixtures for deleting nested children (tests/nested_child_delete.rs):
-- ncd_parents -> ncd_children -> ncd_grandchildren, plus a forgettable child
-- (ncd_secrets) and a child repo with a post-persist hook (ncd_hooked).

CREATE TABLE ncd_parents (
  id UUID PRIMARY KEY,
  deleted BOOL NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE ncd_parent_events (
  id UUID NOT NULL REFERENCES ncd_parents(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ncd_children (
  id UUID PRIMARY KEY,
  parent_id UUID NOT NULL REFERENCES ncd_parents(id),
  label VARCHAR NOT NULL,
  deleted BOOL NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE ncd_child_events (
  id UUID NOT NULL REFERENCES ncd_children(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ncd_grandchildren (
  id UUID PRIMARY KEY,
  child_id UUID NOT NULL REFERENCES ncd_children(id),
  deleted BOOL NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE ncd_grandchild_events (
  id UUID NOT NULL REFERENCES ncd_grandchildren(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ncd_secrets (
  id UUID PRIMARY KEY,
  parent_id UUID NOT NULL REFERENCES ncd_parents(id),
  email VARCHAR,
  deleted BOOL NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE ncd_secret_events (
  id UUID NOT NULL REFERENCES ncd_secrets(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ncd_secrets_forgettable_payloads (
  entity_id UUID NOT NULL REFERENCES ncd_secrets(id),
  sequence INT NOT NULL,
  payload JSONB NOT NULL,
  UNIQUE(entity_id, sequence)
);

CREATE TABLE ncd_hooked (
  id UUID PRIMARY KEY,
  parent_id UUID NOT NULL REFERENCES ncd_parents(id),
  deleted BOOL NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE ncd_hooked_events (
  id UUID NOT NULL REFERENCES ncd_hooked(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
