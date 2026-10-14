-- Nested fixtures for repos that generate their ids. `pallet_items` has no
-- constraint a create or an update can reject on, so a `pallets` write can
-- reject on nothing; `rack_items.sku` is unique, which keeps `racks` writes
-- rejecting even though `racks` itself is a primary key and nothing else.
CREATE TABLE pallets (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE pallet_events (
  id UUID NOT NULL REFERENCES pallets(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
CREATE TABLE pallet_items (
  id UUID PRIMARY KEY,
  pallet_id UUID NOT NULL REFERENCES pallets(id),
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE pallet_item_events (
  id UUID NOT NULL REFERENCES pallet_items(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE racks (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE rack_events (
  id UUID NOT NULL REFERENCES racks(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
CREATE TABLE rack_items (
  id UUID PRIMARY KEY,
  rack_id UUID NOT NULL REFERENCES racks(id),
  sku VARCHAR NOT NULL UNIQUE,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE rack_item_events (
  id UUID NOT NULL REFERENCES rack_items(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
