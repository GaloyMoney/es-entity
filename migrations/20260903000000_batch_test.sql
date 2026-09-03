-- Fixture for tests/batch.rs.
--
-- The batch-isolation tests need a table whose primary key they can violate on
-- demand: a "culprit" item inserts a value the test seeded beforehand, which
-- produces a real Postgres error that would abort the whole transaction without
-- a savepoint. Integer keys keep the probe assertions readable.
CREATE TABLE batch_items (
  v INT PRIMARY KEY
);
