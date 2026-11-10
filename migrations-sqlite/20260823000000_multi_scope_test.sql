CREATE TABLE facilities (
  id BLOB PRIMARY KEY NOT NULL,
  partner_id BLOB NOT NULL,
  customer_id BLOB NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_facilities_partner_created_id ON facilities (partner_id, created_at DESC, id DESC);
CREATE INDEX idx_facilities_customer_created_id ON facilities (customer_id, created_at DESC, id DESC);

CREATE TABLE facility_events (
  id BLOB NOT NULL REFERENCES facilities(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
