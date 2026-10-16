-- Fixtures for `create_or_find_in_op`. `ensure_accounts` has caller-chosen ids,
-- a required unique key and an optional one; `ensure_partials` has a partial
-- unique index, which arbitrates a conflict but cannot be looked up;
-- `ensure_members` is scoped by tenant with a key unique across tenants.
CREATE TABLE ensure_accounts (
  id UUID PRIMARY KEY,
  code VARCHAR NOT NULL UNIQUE,
  external_id VARCHAR UNIQUE,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE ensure_account_events (
  id UUID NOT NULL REFERENCES ensure_accounts(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ensure_partials (
  id UUID PRIMARY KEY,
  tag VARCHAR NOT NULL,
  retired BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE UNIQUE INDEX ensure_partials_live_tag_idx ON ensure_partials (tag) WHERE NOT retired;
CREATE TABLE ensure_partial_events (
  id UUID NOT NULL REFERENCES ensure_partials(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ensure_members (
  id UUID PRIMARY KEY,
  tenant_id UUID NOT NULL,
  email VARCHAR NOT NULL UNIQUE,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE TABLE ensure_member_events (
  id UUID NOT NULL REFERENCES ensure_members(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
