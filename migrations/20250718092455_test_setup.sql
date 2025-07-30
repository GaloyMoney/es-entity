CREATE TABLE users (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_users_name ON users (name);

CREATE TABLE user_events (
  id UUID NOT NULL REFERENCES users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ignore_prefix_users (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_ignore_prefix_users_name ON ignore_prefix_users (name);

CREATE TABLE ignore_prefix_user_events (
  id UUID NOT NULL REFERENCES ignore_prefix_users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE custom_name_for_users (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_custom_name_for_users_name ON custom_name_for_users (name);

CREATE TABLE custom_name_for_user_events (
  id UUID NOT NULL REFERENCES custom_name_for_users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
