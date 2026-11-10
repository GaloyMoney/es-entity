-- Test tables for forgettable index columns.
--
-- `email` is a `Forgettable<String>` column: it is a normal, queryable index
-- column while the subscriber is live, but is set to NULL by `forget()` and by
-- soft `delete()` (auto-forget). It must therefore be nullable.
CREATE TABLE subscribers (
  id BLOB PRIMARY KEY NOT NULL,
  email TEXT,
  plan TEXT NOT NULL,
  created_at TEXT NOT NULL,
  deleted BOOLEAN NOT NULL DEFAULT FALSE
);

CREATE TABLE subscriber_events (
  id BLOB NOT NULL REFERENCES subscribers(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE subscribers_forgettable_payloads (
  entity_id BLOB NOT NULL REFERENCES subscribers(id),
  sequence INTEGER NOT NULL,
  payload TEXT NOT NULL,
  UNIQUE(entity_id, sequence)
);
