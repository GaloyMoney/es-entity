-- CHECK-constraint fixture for tests/repo_errors.rs.
--
-- The repo error classifier maps any database error that names a constraint
-- (unique, foreign key, check, exclusion) to the typed `ConstraintViolation`
-- variant. This CHECK constraint exercises the non-unique path: creating or
-- updating a profile with a blank email surfaces as
-- `ConstraintViolation { column: None, .. }` since the constraint name is not
-- a recognized unique index of the `email` column.
--
-- SQLite translation: SQLite has no `ALTER TABLE ... ADD CONSTRAINT`, so the
-- Postgres `ALTER TABLE profiles ADD CONSTRAINT profiles_email_not_blank
-- CHECK (email <> '')` is applied by rebuilding `profiles` (create new table
-- with the named CHECK, copy rows, drop old, rename, recreate its index).
-- `profile_events` references `profiles` by name, so its foreign key resolves
-- to the rebuilt table once the rename completes. The table is empty when
-- migrations run, so the implicit DELETE of DROP TABLE cannot violate FKs.
CREATE TABLE profiles_new (
  id BLOB PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  display_name TEXT NOT NULL,
  email TEXT NOT NULL,
  created_at TEXT NOT NULL,
  CONSTRAINT profiles_email_not_blank CHECK (email <> '')
);
INSERT INTO profiles_new (id, name, display_name, email, created_at)
  SELECT id, name, display_name, email, created_at FROM profiles;
DROP TABLE profiles;
ALTER TABLE profiles_new RENAME TO profiles;
CREATE UNIQUE INDEX idx_profiles_email ON profiles (email);
