CREATE TABLE contacts (
  id BLOB PRIMARY KEY NOT NULL,
  partner_id BLOB NOT NULL,
  email TEXT NOT NULL,
  status TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_contacts_partner_created_id ON contacts (partner_id, created_at DESC, id DESC);
CREATE INDEX idx_contacts_partner_email ON contacts (partner_id, email);

CREATE TABLE contact_events (
  id BLOB NOT NULL REFERENCES contacts(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
