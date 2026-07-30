CREATE TABLE contacts (
  id UUID PRIMARY KEY,
  partner_id UUID NOT NULL,
  email VARCHAR NOT NULL,
  status VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_contacts_partner_created_id ON contacts (partner_id, created_at DESC, id DESC);
CREATE INDEX idx_contacts_partner_email ON contacts (partner_id, email);

CREATE TABLE contact_events (
  id UUID NOT NULL REFERENCES contacts(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
