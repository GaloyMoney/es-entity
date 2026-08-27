-- Dedicated tables for a `list_by_id` paging test (peeked-but-unreturned
-- entity must not cause a false `StaleAggregateRead`).
--
-- Deliberately NOT reusing `orders`/`order_items`: those tables are shared,
-- unscoped, across every test in every file that imports `entities::order`,
-- and this test pages the *whole* table with no filter — sharing would make
-- it flaky for reasons that have nothing to do with the bug it proves.
CREATE TABLE paging_orders (
  id UUID PRIMARY KEY,
  version INT NOT NULL DEFAULT 1,
  deleted BOOL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE paging_order_events (
  id UUID NOT NULL REFERENCES paging_orders(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE paging_order_items (
  id UUID PRIMARY KEY,
  order_id UUID NOT NULL REFERENCES paging_orders(id),
  deleted BOOL DEFAULT false,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE paging_order_item_events (
  id UUID NOT NULL REFERENCES paging_order_items(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
