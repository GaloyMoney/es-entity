CREATE TABLE transfers (
  id BLOB PRIMARY KEY NOT NULL,
  account_id BLOB NOT NULL,
  status TEXT NOT NULL,
  reference TEXT DEFAULT NULL,
  score INTEGER DEFAULT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_transfers_account_created_id ON transfers (account_id, created_at DESC, id DESC);
CREATE INDEX idx_transfers_status ON transfers (status);
CREATE INDEX idx_transfers_score_id ON transfers (score, id);

CREATE TABLE transfer_events (
  id BLOB NOT NULL REFERENCES transfers(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
