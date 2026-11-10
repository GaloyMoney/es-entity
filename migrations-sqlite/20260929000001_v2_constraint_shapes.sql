-- Independent fixture for typed composite keys and partial-index semantics.
CREATE TABLE v2_profiles (
  id BLOB PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  email TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX v2_profiles_active_identity ON v2_profiles(name, email)
  WHERE name <> 'inactive';
CREATE TABLE v2_profile_events (
  id BLOB NOT NULL REFERENCES v2_profiles(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
