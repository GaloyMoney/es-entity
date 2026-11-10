-- Fixture for tests/batch.rs.
--
-- The batch-isolation tests need a table whose primary key they can violate on
-- demand: a "culprit" item inserts a value the test seeded beforehand, which
-- produces a real database error that would abort the whole transaction without
-- a savepoint. Integer keys keep the probe assertions readable.
--
-- SQLite translation: `INTEGER PRIMARY KEY` makes `v` an alias of the rowid;
-- explicit duplicate values still raise a PRIMARY KEY constraint error.
CREATE TABLE batch_items (
  v INTEGER PRIMARY KEY
);
