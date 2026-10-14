-- Fixtures for repos that generate their own ids (the default id policy).
-- `gadgets`, `sprockets` and `doohickeys` carry nothing but a primary key, so
-- their creates have no constraint left to reject on; `knobs` adds a unique
-- column, which stays in the family.
CREATE TABLE gadgets (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE gadget_events (
  id UUID NOT NULL REFERENCES gadgets(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE knobs (
  id UUID PRIMARY KEY,
  serial VARCHAR NOT NULL UNIQUE,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE knob_events (
  id UUID NOT NULL REFERENCES knobs(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE sprockets (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE sprocket_events (
  id UUID NOT NULL REFERENCES sprockets(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE doohickeys (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE doohickey_events (
  id UUID NOT NULL REFERENCES doohickeys(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
