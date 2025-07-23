CREATE TABLE users (
  id UUID PRIMARY KEY,                   -- Mandatory id column
  created_at TIMESTAMPTZ NOT NULL,       -- Mandatory created_at column

  name VARCHAR UNIQUE NULL               -- Any other columns you want a quick 'index-based' lookup
);

CREATE TABLE user_events (               -- The table that actually stores the events sequenced per entity
  id UUID NOT NULL REFERENCES users(id), -- This table has the same columns for every entity you create (by convention named `<entity>_events`).
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);
