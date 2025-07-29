-- Add migration script here
CREATE TABLE test_users (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_test_users_name ON test_users (name);

CREATE TABLE test_user_events (
  id UUID NOT NULL REFERENCES test_users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
