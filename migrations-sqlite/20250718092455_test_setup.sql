CREATE TABLE users (
  id BLOB PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_users_name ON users (name);

CREATE TABLE user_events (
  id BLOB NOT NULL REFERENCES users(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE user_documents (
  id BLOB PRIMARY KEY NOT NULL,
  user_id BLOB,
  created_at TEXT NOT NULL
);

CREATE TABLE user_document_events (
  id BLOB NOT NULL REFERENCES user_documents(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ignore_prefix_users (
  id BLOB PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_ignore_prefix_users_name ON ignore_prefix_users (name);

CREATE TABLE ignore_prefix_user_events (
  id BLOB NOT NULL REFERENCES ignore_prefix_users(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE custom_name_for_users (
  id BLOB PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_custom_name_for_users_name ON custom_name_for_users (name);

CREATE TABLE custom_name_for_user_events (
  id BLOB NOT NULL REFERENCES custom_name_for_users(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

-- Tables for nested entities test
CREATE TABLE orders (
  id BLOB PRIMARY KEY NOT NULL,
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);

CREATE TABLE order_events (
  id BLOB NOT NULL REFERENCES orders(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE order_items (
  id BLOB PRIMARY KEY NOT NULL,
  order_id BLOB NOT NULL REFERENCES orders(id),
  deleted BOOLEAN DEFAULT FALSE,
  created_at TEXT NOT NULL
);

CREATE TABLE order_item_events (
  id BLOB NOT NULL REFERENCES order_items(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

-- Tables for subscription/billing period example
CREATE TABLE subscriptions (
  id BLOB PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE subscription_events (
  id BLOB NOT NULL REFERENCES subscriptions(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE billing_periods (
  id BLOB PRIMARY KEY NOT NULL,
  subscription_id BLOB NOT NULL REFERENCES subscriptions(id),
  created_at TEXT NOT NULL
);

CREATE TABLE billing_period_events (
  id BLOB NOT NULL REFERENCES billing_periods(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE hook_events (
  entity_id BLOB NOT NULL,
  event_type TEXT NOT NULL,
  created_at TEXT NOT NULL
);

-- Tables for custom accessor tests
CREATE TABLE profiles (
  id BLOB PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  display_name TEXT NOT NULL,
  email TEXT NOT NULL,
  created_at TEXT NOT NULL
);
-- Deliberately NOT the `{table}_{column}_key` convention name: the duplicate
-- error tests in tests/repo_errors.rs then map `email` to its column *only*
-- via the migration-derived index catalog (the former `constraint = "…"`
-- attribute), proving that path end-to-end.
CREATE UNIQUE INDEX idx_profiles_email ON profiles (email);

CREATE TABLE profile_events (
  id BLOB NOT NULL REFERENCES profiles(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

-- Tables for forgettable payloads test
CREATE TABLE customers (
  id BLOB PRIMARY KEY NOT NULL,
  email TEXT NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE customer_events (
  id BLOB NOT NULL REFERENCES customers(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE customers_forgettable_payloads (
  entity_id BLOB NOT NULL REFERENCES customers(id),
  sequence INTEGER NOT NULL,
  payload TEXT NOT NULL,
  UNIQUE(entity_id, sequence)
);

-- Parent + forgettable nested child, for cascade-delete PII scrub coverage.
--
-- `account_holders` is a nested child of `accounts` (soft-delete parent). It has
-- both a `Forgettable<String>` event field (`name`, stored in the payloads
-- table) and a `Forgettable<String>` index column (`email`, nullable). Soft-
-- deleting the parent must scrub both: delete the child payload rows and NULL
-- the child index column.
CREATE TABLE accounts (
  id BLOB PRIMARY KEY NOT NULL,
  deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL
);

CREATE TABLE account_events (
  id BLOB NOT NULL REFERENCES accounts(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE account_holders (
  id BLOB PRIMARY KEY NOT NULL,
  account_id BLOB NOT NULL REFERENCES accounts(id),
  email TEXT,
  deleted BOOLEAN NOT NULL DEFAULT FALSE,
  created_at TEXT NOT NULL
);

CREATE TABLE account_holder_events (
  id BLOB NOT NULL REFERENCES account_holders(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE account_holders_forgettable_payloads (
  entity_id BLOB NOT NULL REFERENCES account_holders(id),
  sequence INTEGER NOT NULL,
  payload TEXT NOT NULL,
  UNIQUE(entity_id, sequence)
);
