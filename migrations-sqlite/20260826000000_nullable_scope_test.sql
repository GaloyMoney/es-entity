-- Mirrors lana's core/party shape: `customer_id` is a scope column whose
-- Rust type is `Option<CustomerId>`. NULL encodes "no customer owns this row
-- directly" (e.g. an organization-member individual party) — such rows must
-- be invisible to every `Customer(_)` scoped read and visible only under
-- `All`.
CREATE TABLE parties (
  id BLOB PRIMARY KEY NOT NULL,
  customer_id BLOB,
  name TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_parties_customer_created_id ON parties (customer_id, created_at DESC, id DESC);

CREATE TABLE party_events (
  id BLOB NOT NULL REFERENCES parties(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
