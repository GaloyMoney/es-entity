-- Three-level nesting (parent -> child -> grandchild), used to prove nested
-- batching composes to arbitrary depth (see tests/nested_grandchildren.rs).

CREATE TABLE gc_parents (
  id BLOB PRIMARY KEY NOT NULL,
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);

CREATE TABLE gc_parent_events (
  id BLOB NOT NULL REFERENCES gc_parents(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE gc_children (
  id BLOB PRIMARY KEY NOT NULL,
  parent_id BLOB NOT NULL REFERENCES gc_parents(id),
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);

CREATE TABLE gc_child_events (
  id BLOB NOT NULL REFERENCES gc_children(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE gc_grandchildren (
  id BLOB PRIMARY KEY NOT NULL,
  child_id BLOB NOT NULL REFERENCES gc_children(id),
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);

CREATE TABLE gc_grandchild_events (
  id BLOB NOT NULL REFERENCES gc_grandchildren(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
