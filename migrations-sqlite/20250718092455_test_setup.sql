CREATE TABLE users (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  deleted INTEGER DEFAULT 0,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_users_name ON users (name);

CREATE TABLE user_events (
  id TEXT NOT NULL REFERENCES users(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE user_documents (
  id TEXT PRIMARY KEY NOT NULL,
  user_id TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE user_document_events (
  id TEXT NOT NULL REFERENCES user_documents(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ignore_prefix_users (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_ignore_prefix_users_name ON ignore_prefix_users (name);

CREATE TABLE ignore_prefix_user_events (
  id TEXT NOT NULL REFERENCES ignore_prefix_users(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE custom_name_for_users (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX idx_custom_name_for_users_name ON custom_name_for_users (name);

CREATE TABLE custom_name_for_user_events (
  id TEXT NOT NULL REFERENCES custom_name_for_users(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

-- Tables for nested entities test
CREATE TABLE orders (
  id TEXT PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE order_events (
  id TEXT NOT NULL REFERENCES orders(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE order_items (
  id TEXT PRIMARY KEY NOT NULL,
  order_id TEXT NOT NULL REFERENCES orders(id),
  created_at TEXT NOT NULL
);

CREATE TABLE order_item_events (
  id TEXT NOT NULL REFERENCES order_items(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

-- Tables for subscription/billing period example
CREATE TABLE subscriptions (
  id TEXT PRIMARY KEY NOT NULL,
  created_at TEXT NOT NULL
);

CREATE TABLE subscription_events (
  id TEXT NOT NULL REFERENCES subscriptions(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE billing_periods (
  id TEXT PRIMARY KEY NOT NULL,
  subscription_id TEXT NOT NULL REFERENCES subscriptions(id),
  created_at TEXT NOT NULL
);

CREATE TABLE billing_period_events (
  id TEXT NOT NULL REFERENCES billing_periods(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE hook_events (
  entity_id TEXT NOT NULL,
  event_type TEXT NOT NULL,
  created_at TEXT NOT NULL
);

-- Tables for custom accessor tests
CREATE TABLE profiles (
  id TEXT PRIMARY KEY NOT NULL,
  name TEXT NOT NULL,
  display_name TEXT NOT NULL,
  email TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE UNIQUE INDEX profiles_email_key ON profiles (email);

CREATE TABLE profile_events (
  id TEXT NOT NULL REFERENCES profiles(id),
  sequence INTEGER NOT NULL,
  event_type TEXT NOT NULL,
  event TEXT NOT NULL,
  context TEXT DEFAULT NULL,
  recorded_at TEXT NOT NULL,
  UNIQUE(id, sequence)
);
