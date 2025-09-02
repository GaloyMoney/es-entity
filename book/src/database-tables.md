# Database Tables

In `es-entity` every `Entity` gets 2 tables - the `index` and the `events` table.
This section will explain the rational behind that.

## The Events Table
The events table always has the same fields:

```sql
CREATE TABLE user_events (
  -- The entity id
  id UUID NOT NULL REFERENCES users(id),
  -- The sequence number of the event
  -- Starting at 1 and incrementing by 1 each event
  sequence INT NOT NULL,
  -- The 'type' of the event (corresponding to the enum variant)
  event_type VARCHAR NOT NULL,
  -- The event data serialized as a JSON blob
  event JSONB NOT NULL,
  -- The 'event context'
  -- additional metadata that can be collected out of band
  context JSONB DEFAULT NULL,
  -- The time the event was recorded
  recorded_at TIMESTAMPTZ NOT NULL,
  -- Unique constraint to ensure there are no duplicate sequence numbers
  UNIQUE(id, sequence)
);
```

In fact we could persist all events to a global table with that schema but partitioning the events per `Entity` gives us some benefits when querying (like read performance and referential integrity).

Intuitively you might think this is all we need as we can very easily query all the events for a specific `Entity`:
```sql
SELECT * FROM user_events WHERE id = $1 ORDER BY sequence
```

This is correct if we know the `id` of the `Entity` we are looking up.
But it becomes a lot more tricky when we want to do a lookup on a non-id field.

Assuming the `Event`-`enum` looks like this:
```rust
pub enum UserEvent {
    Initialized { id: u64, name: String },
    NameUpdated { name: String },
    EmailUpdated { email: String },
}
```

and we want to lookup a user by email, the query would quickly become a lot more complicated.
Lets consider the naive query:
```sql
SELECT * FROM user_events WHERE event->>'email' = $1;
```

This doesn't work as it only gets a single event - but we want _all_ events for that `Entity`.
```sql
SELECT *
FROM user_events
WHERE id = (
  SELECT id
  FROM user_events
  WHERE event->>'email' = $1
  LIMIT 1
)
ORDER BY sequence;
```
This also doesn't work because perhaps the event that was found wasn't the latest `EmailUpdated` event in the `User`s history.
But we want to get the user who's email is _currently_ `$1`.
So it could find some false positives.

When iterating with ChatGPT the next suggestion is:
```sql
WITH latest_email_updates AS (
  SELECT id, MAX(sequence) AS max_sequence
  FROM user_events
  WHERE event_type = 'email_updated'
  GROUP BY id
),
latest_emails AS (
  SELECT e.id, e.event->>'email' AS email
  FROM user_events e
  JOIN latest_email_updates leu
    ON e.id = leu.id AND e.sequence = leu.max_sequence
  WHERE e.event_type = 'email_updated'
),
target_user AS (
  SELECT id
  FROM latest_emails
  WHERE email = $1
)
SELECT *
FROM user_events
WHERE id = (SELECT id FROM target_user)
ORDER BY sequence;
```

This query might execute what we want but it still has issues.
The worst one being that we are leaking a lot of domain knowledge into the query.
Specifically the presence and shape of the `EmailUpdated` event is encoded into the query.
Preferably the specifics of the `Event`-schemas would only need to be known on the domain side encoded in the `EntityEvent` enum.

Also the whole query is quite inefficient.
Sure we could add an index on the `event->>'email'` field but that would introduce more implicit coupling.
Also what if we wanted something like a `UNIQUE` constraint on the email - but still allow emails swapped multiple times.

## The Index Table

Enter the `index` table.
The `index`-table is a table that hosts 1 row per `Entity` with the columns populated by the latest values.
In that sense it looks very similar to a table that might hold the entire state of the `Entity` in a typical `update-in-place` persistence strategy.
The difference is that we _only_ include columns that we want to `index` for fast lookup or some kind of constraint like `UNIQUE` or `REFERENCES`.
In that sense it is purely an optimization and does not represent the entire state of the `Entity` - for that you must load all the events.

```sql
CREATE TABLE users (
  id UUID PRIMARY KEY,
  created_at TIMESTAMPTZ NOT NULL,

  email VARCHAR UNIQUE
);
```

Now the query simplifies to:
```sql
WITH target_entity AS (
  SELECT id
  FROM users
  WHERE email = $1
)
SELECT e.*
FROM user_events e
JOIN target_entity te ON e.id = te.id
ORDER BY e.sequence;
```

As a result the query is much simpler and we are no longer leaking any domain information.
We just have to ensure the index table gets updated atomically as we append the events to the events table.
