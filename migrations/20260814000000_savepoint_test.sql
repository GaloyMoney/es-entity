-- Fixture for tests/savepoint.rs.
--
-- The savepoint tests write raw rows to assert what a rolled-back item leaves
-- behind, so they need a table of their own: writing to a shared entity table
-- (e.g. `users`) would leave rows with no corresponding events and break the
-- hydration-based tests. The primary key is what the poisoning test violates to
-- produce an error that would otherwise abort the whole transaction.
CREATE TABLE savepoint_items (
  id UUID PRIMARY KEY,
  label VARCHAR NOT NULL
);
