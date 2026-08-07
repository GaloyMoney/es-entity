-- CHECK-constraint fixture for tests/repo_errors.rs.
--
-- The repo error classifier maps any database error that names a constraint
-- (unique, foreign key, check, exclusion) to the typed `ConstraintViolation`
-- variant. This CHECK constraint exercises the non-unique path: creating or
-- updating a profile with a blank email surfaces as
-- `ConstraintViolation { column: None, .. }` since the constraint name is not
-- a recognized unique index of the `email` column.
ALTER TABLE profiles
  ADD CONSTRAINT profiles_email_not_blank CHECK (email <> '');
