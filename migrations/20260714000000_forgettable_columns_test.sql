-- Test tables for forgettable index columns.
--
-- `email` is a `Forgettable<String>` column: it is a normal, queryable index
-- column while the subscriber is live, but is set to NULL by `forget()` and by
-- soft `delete()` (auto-forget). It must therefore be nullable.
CREATE TABLE subscribers (
  id UUID PRIMARY KEY,
  email VARCHAR,
  plan VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL,
  deleted BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE subscriber_events (
  id UUID NOT NULL REFERENCES subscribers(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE subscribers_forgettable_payloads (
  entity_id UUID NOT NULL REFERENCES subscribers(id),
  sequence INT NOT NULL,
  payload JSONB NOT NULL,
  UNIQUE(entity_id, sequence)
);
