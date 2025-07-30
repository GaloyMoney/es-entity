CREATE TABLE customers (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_customers_name ON customers (name);

CREATE TABLE customer_events (
  id UUID NOT NULL REFERENCES customers(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE test_customers (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_test_customers_name ON test_customers (name);

CREATE TABLE test_customer_events (
  id UUID NOT NULL REFERENCES test_customers(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
