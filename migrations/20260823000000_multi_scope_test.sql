CREATE TABLE facilities (
  id UUID PRIMARY KEY,
  partner_id UUID NOT NULL,
  customer_id UUID NOT NULL,
  status VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_facilities_partner_created_id ON facilities (partner_id, created_at DESC, id DESC);
CREATE INDEX idx_facilities_customer_created_id ON facilities (customer_id, created_at DESC, id DESC);

CREATE TABLE facility_events (
  id UUID NOT NULL REFERENCES facilities(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
