-- Fixture for tests/unnamed_statement.rs: a custom (non built-in) type whose
-- OID sqlx must resolve through a catalog lookup on a connection that has not
-- seen it yet.
CREATE TYPE unnamed_statement_mood AS ENUM ('happy', 'sad');
