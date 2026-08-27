-- Fixture for `forget()` cascading into nested children.
--
-- `households` is a `forgettable` parent with a nested, also-`forgettable`
-- child `household_members`. This is deliberately a fresh pair of tables
-- rather than reusing `accounts`/`account_holders` (which already covers
-- cascade-*delete* PII scrub in an already-applied migration): forgetting a
-- parent must scrub a direct child's forgettable data too, without touching
-- the child's `deleted` flag — a distinct code path from the delete cascade.
-- `households` is an aggregate root (nested `members`, no `parent` column),
-- so it needs the `version` clock introduced alongside this migration.
CREATE TABLE households (
  id UUID PRIMARY KEY,
  label VARCHAR,
  version INT NOT NULL DEFAULT 1,
  deleted BOOL NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE household_events (
  id UUID NOT NULL REFERENCES households(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE households_forgettable_payloads (
  entity_id UUID NOT NULL REFERENCES households(id),
  sequence INT NOT NULL,
  payload JSONB NOT NULL,
  UNIQUE(entity_id, sequence)
);

CREATE TABLE household_members (
  id UUID PRIMARY KEY,
  household_id UUID NOT NULL REFERENCES households(id),
  email VARCHAR,
  deleted BOOL NOT NULL DEFAULT FALSE,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE household_member_events (
  id UUID NOT NULL REFERENCES household_members(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE household_members_forgettable_payloads (
  entity_id UUID NOT NULL REFERENCES household_members(id),
  sequence INT NOT NULL,
  payload JSONB NOT NULL,
  UNIQUE(entity_id, sequence)
);
