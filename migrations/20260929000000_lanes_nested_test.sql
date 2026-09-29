-- Nested-aggregate fixture for tests/lanes.rs: a child-level unique
-- constraint (`sku`) that a domain rejection hoists through the parent's
-- constraint path (`ParentConstraint::Children(ChildConstraint::SkuKey)`).

CREATE TABLE lane_parents (
  id UUID PRIMARY KEY,
  deleted BOOL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE lane_parent_events (
  id UUID NOT NULL REFERENCES lane_parents(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE lane_items (
  id UUID PRIMARY KEY,
  parent_id UUID NOT NULL REFERENCES lane_parents(id),
  sku VARCHAR NOT NULL UNIQUE,
  deleted BOOL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE lane_item_events (
  id UUID NOT NULL REFERENCES lane_items(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
