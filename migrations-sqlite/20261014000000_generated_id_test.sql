-- Fixtures for repos that generate their own ids (the default id policy).
-- `gadgets`, `sprockets` and `doohickeys` carry nothing but a primary key, so
-- their creates have no constraint left to reject on; `knobs` adds a unique
-- column, which stays in the family.
CREATE TABLE gadgets (
  id BLOB PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE gadget_events (
  id BLOB NOT NULL REFERENCES gadgets(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE knobs (
  id BLOB PRIMARY KEY NOT NULL,
  serial TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);
CREATE TABLE knob_events (
  id BLOB NOT NULL REFERENCES knobs(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE sprockets (
  id BLOB PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE sprocket_events (
  id BLOB NOT NULL REFERENCES sprockets(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE doohickeys (
  id BLOB PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE doohickey_events (
  id BLOB NOT NULL REFERENCES doohickeys(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
