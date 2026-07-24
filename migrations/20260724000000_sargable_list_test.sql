CREATE TABLE transfers (
  id UUID PRIMARY KEY,
  account_id UUID NOT NULL,
  status VARCHAR NOT NULL,
  reference VARCHAR DEFAULT NULL,
  score INT DEFAULT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_transfers_account_created_id ON transfers (account_id, created_at DESC, id DESC);
CREATE INDEX idx_transfers_status ON transfers (status);
CREATE INDEX idx_transfers_score_id ON transfers (score, id);

CREATE TABLE transfer_events (
  id UUID NOT NULL REFERENCES transfers(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
