-- Fixtures for `create_or_find_in_op`. `ensure_accounts` has caller-chosen ids,
-- a required unique key and an optional one; `ensure_partials` has a partial
-- unique index, which arbitrates a conflict but cannot be looked up;
-- `ensure_members` is scoped by tenant with a key unique across tenants.
CREATE TABLE ensure_accounts (
  id BLOB PRIMARY KEY NOT NULL,
  code TEXT NOT NULL UNIQUE,
  external_id TEXT UNIQUE,
  created_at TEXT NOT NULL
);
CREATE TABLE ensure_account_events (
  id BLOB NOT NULL REFERENCES ensure_accounts(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ensure_partials (
  id BLOB PRIMARY KEY NOT NULL,
  tag TEXT NOT NULL,
  retired BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX ensure_partials_live_tag_idx ON ensure_partials (tag) WHERE NOT retired;
CREATE TABLE ensure_partial_events (
  id BLOB NOT NULL REFERENCES ensure_partials(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ensure_members (
  id BLOB PRIMARY KEY NOT NULL,
  tenant_id BLOB NOT NULL,
  email TEXT NOT NULL UNIQUE,
  created_at TEXT NOT NULL
);
CREATE TABLE ensure_member_events (
  id BLOB NOT NULL REFERENCES ensure_members(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
