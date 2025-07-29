# Event Sourcing

Event sourcing is a software design pattern where state changes are stored as a sequence of events rather than as snapshots that get updated in place.

Instead of updating a database record directly, every change is recorded as an immutable event (e.g., UserCreated, EmailChanged, AccountDeactivated) which gets inserted as a row in a table.
The current state is rebuilt by replaying these events in order.

One thing to note is that in `es-entity` the events are scoped to a specific type of `Entity`.
Thus they are (by convention) not all written to the same global events table like in some Event Sourcing approaches.
Rather each `Entity`-type gets its own `events` table - though it is possible to use a global table if desired.
Further the events are strictly ordered on a per-entity basis - there are no ordering guarantees across `entities`.

This can be interpreted as the `Entity`-type representing a `topic` and the `EntityId` playing the role of the `PartitionKey` that exists in some pub-sub / event-store systems.
