-- Nested fixtures for repos that generate their ids. `pallet_items` has no
-- constraint a create or an update can reject on, so a `pallets` write can
-- reject on nothing; `rack_items.sku` is unique, which keeps `racks` writes
-- rejecting even though `racks` itself is a primary key and nothing else.
CREATE TABLE pallets (
  id BLOB PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE pallet_events (
  id BLOB NOT NULL REFERENCES pallets(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
CREATE TABLE pallet_items (
  id BLOB PRIMARY KEY NOT NULL,
  pallet_id BLOB NOT NULL REFERENCES pallets(id),
  created_at TEXT NOT NULL
);
CREATE TABLE pallet_item_events (
  id BLOB NOT NULL REFERENCES pallet_items(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE racks (
  id BLOB PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);
CREATE TABLE rack_events (
  id BLOB NOT NULL REFERENCES racks(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
CREATE TABLE rack_items (
  id BLOB PRIMARY KEY NOT NULL,
  rack_id BLOB NOT NULL REFERENCES racks(id),
  sku TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);
CREATE TABLE rack_item_events (
  id BLOB NOT NULL REFERENCES rack_items(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
