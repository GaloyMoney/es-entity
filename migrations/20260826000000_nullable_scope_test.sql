-- Mirrors lana's core/party shape: `customer_id` is a scope column whose
-- Rust type is `Option<CustomerId>`. NULL encodes "no customer owns this row
-- directly" (e.g. an organization-member individual party) — such rows must
-- be invisible to every `Customer(_)` scoped read and visible only under
-- `All`.
CREATE TABLE parties (
  id UUID PRIMARY KEY,
  customer_id UUID,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_parties_customer_created_id ON parties (customer_id, created_at DESC, id DESC);

CREATE TABLE party_events (
  id UUID NOT NULL REFERENCES parties(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
