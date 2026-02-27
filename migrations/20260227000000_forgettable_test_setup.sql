-- Tables for forgettable payloads test
CREATE TABLE customers (
  id UUID PRIMARY KEY,
  email VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE customer_events (
  id UUID NOT NULL REFERENCES customers(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE customers_forgettable_payloads (
  entity_id UUID NOT NULL REFERENCES customers(id),
  sequence INT NOT NULL,
  payload JSONB NOT NULL,
  UNIQUE(entity_id, sequence)
);
