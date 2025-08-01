CREATE TABLE users (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  deleted BOOL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_users_name ON users (name);

CREATE TABLE user_events (
  id UUID NOT NULL REFERENCES users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE user_documents (
  id UUID PRIMARY KEY,
  user_id UUID,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE user_document_events (
  id UUID NOT NULL REFERENCES user_documents(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE ignore_prefix_users (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_ignore_prefix_users_name ON ignore_prefix_users (name);

CREATE TABLE ignore_prefix_user_events (
  id UUID NOT NULL REFERENCES ignore_prefix_users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE custom_name_for_users (
  id UUID PRIMARY KEY,
  name VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_custom_name_for_users_name ON custom_name_for_users (name);

CREATE TABLE custom_name_for_user_events (
  id UUID NOT NULL REFERENCES custom_name_for_users(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

-- Tables for nested entities test
CREATE TABLE orders (
  id UUID PRIMARY KEY,
  customer_name VARCHAR NOT NULL,
  status VARCHAR NOT NULL DEFAULT 'pending',
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE order_events (
  id UUID NOT NULL REFERENCES orders(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE order_items (
  id UUID PRIMARY KEY,
  order_id UUID NOT NULL REFERENCES orders(id),
  product_name VARCHAR NOT NULL,
  quantity INTEGER NOT NULL,
  price DECIMAL(10,2) NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX idx_order_items_order_id ON order_items (order_id);

CREATE TABLE order_item_events (
  id UUID NOT NULL REFERENCES order_items(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
