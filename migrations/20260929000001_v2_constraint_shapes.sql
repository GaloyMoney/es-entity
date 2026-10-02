-- Independent fixture for typed composite keys and partial-index semantics.
CREATE TABLE v2_profiles (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  email VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE UNIQUE INDEX v2_profiles_active_identity ON v2_profiles(name, email)
  WHERE name <> 'inactive';
CREATE TABLE v2_profile_events (
  id UUID NOT NULL REFERENCES v2_profiles(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
