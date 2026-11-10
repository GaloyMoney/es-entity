-- Nested-aggregate fixture for tests/lanes.rs: a child-level unique
-- constraint (`sku`) that a domain rejection hoists through the parent's
-- constraint path (`ParentConstraint::Children(ChildConstraint::SkuKey)`).

CREATE TABLE lane_parents (
  id BLOB PRIMARY KEY NOT NULL,
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);

CREATE TABLE lane_parent_events (
  id BLOB NOT NULL REFERENCES lane_parents(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE lane_items (
  id BLOB PRIMARY KEY NOT NULL,
  parent_id BLOB NOT NULL REFERENCES lane_parents(id),
  sku TEXT NOT NULL UNIQUE,
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);

CREATE TABLE lane_item_events (
  id BLOB NOT NULL REFERENCES lane_items(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
