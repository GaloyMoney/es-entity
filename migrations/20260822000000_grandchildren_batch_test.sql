-- Three-level nesting (parent -> child -> grandchild), used to prove nested
-- batching composes to arbitrary depth (see tests/nested_grandchildren.rs).

CREATE TABLE gc_parents (
  id UUID PRIMARY KEY,
  deleted BOOL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE gc_parent_events (
  id UUID NOT NULL REFERENCES gc_parents(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE gc_children (
  id UUID PRIMARY KEY,
  parent_id UUID NOT NULL REFERENCES gc_parents(id),
  deleted BOOL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE gc_child_events (
  id UUID NOT NULL REFERENCES gc_children(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE gc_grandchildren (
  id UUID PRIMARY KEY,
  child_id UUID NOT NULL REFERENCES gc_children(id),
  deleted BOOL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE gc_grandchild_events (
  id UUID NOT NULL REFERENCES gc_grandchildren(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
