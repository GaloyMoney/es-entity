# Snapshots

Loading a long-lived aggregate by replaying every event it has ever emitted gets
expensive as the stream grows. `#[es_repo(snapshot)]` lets a repo persist a
periodic fold of an entity's state — a **snapshot** — in the same statement as
the events it appends, so every loader (flat or nested) reads the snapshot plus
only the events *after* it, in **one round trip**.

## How It Works

1. Your entity's `events` field widens from `EntityEvents<E>` to
   `EntityEvents<E, S>`, where `S` is your own state type — the fold of events
   `1..=k` as of some sequence `k`.
2. You implement `Snapshotting::snapshot(&self) -> Option<S>`, business logic
   that decides, on every write, whether the current state is worth persisting
   as the new snapshot head.
3. The repo's `create`/`update`/`create_all`/`update_all` functions call
   `snapshot()` before their write statement and, if it returns `Some`, upsert
   the state into a `<tbl>_snapshots` table in the *same* statement as the
   event insert — no extra round trip.
4. Every loader reads `<tbl>_snapshots` (matched by a **fingerprint** — see
   below) left-joined to only the events after it, and hands your entity a
   `Replay` stream: the snapshot first (if any), then the tail of events after
   it.
5. In memory, `EntityEvents<E, S>` compacts itself right after a write that
   took a snapshot: the tail is dropped and replaced by the just-written
   state. A freshly-loaded entity and a freshly-updated one look identical.

A repo *without* `snapshot` is completely unaffected: its generated SQL and
`.sqlx` cache are byte-for-byte the same as before this feature existed.

## Database Setup

Alongside your existing table and events table, add a `<tbl>_snapshots` table:

```sql
CREATE TABLE meters (
  id UUID PRIMARY KEY,
  label VARCHAR NOT NULL,
  created_at TIMESTAMPTZ NOT NULL
);

CREATE TABLE meter_events (
  id UUID NOT NULL REFERENCES meters(id),
  sequence INT NOT NULL,
  event_type VARCHAR NOT NULL,
  event JSONB NOT NULL,
  context JSONB DEFAULT NULL,
  recorded_at TIMESTAMPTZ NOT NULL,
  UNIQUE(id, sequence)
);

CREATE TABLE meter_snapshots (
  id UUID PRIMARY KEY REFERENCES meters(id),
  sequence INT NOT NULL,
  fingerprint BIGINT NOT NULL,
  snapshot JSONB NOT NULL,
  first_recorded_at TIMESTAMPTZ NOT NULL,
  recorded_at TIMESTAMPTZ NOT NULL
);
```

`sequence` is the last event sequence folded into `snapshot` — the tail read
by a loader is exactly `WHERE e.sequence > snapshot.sequence`.

## Defining a Snapshot Type

`#[derive(EsSnapshot)]` on a plain struct: every field is part of your fold.

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# use serde::{Deserialize, Serialize};
use es_entity::*;

es_entity::entity_id! { MeterId }

#[derive(EsSnapshot, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[es_snapshot(version = 1)]
pub struct MeterSnapshot {
    pub id: MeterId,
    pub label: String,
    pub count: u32,
    pub total: i64,
}
# fn main() {}
```

`#[es_snapshot(version = N)]` (default `0`) salts a **fingerprint**: an FNV-1a
hash over the struct's field names, types, and version, computed once at
macro-expansion time and embedded as a literal `i64`. Every loader binds this
value; a stored snapshot only joins when its own fingerprint matches. Change
the shape of `MeterSnapshot` (add/remove/rename/retype a field) and the
fingerprint changes with it — old rows simply stop matching and every loader
falls back to a full replay for that entity, self-healing the next time it
writes a fresh snapshot. Bump `version` by hand for a change the derive can't
see (e.g. a change in how a field is *interpreted*, not its Rust type).

A fingerprint mismatch is never an error — it's the mechanism that makes
schema evolution safe without a migration step.

## The `Snapshotting` Trait and `Replay`

Your entity implements `Snapshotting::snapshot()`, and every place that used to
fold over `events.iter_all()` switches to `events.replay()`, which yields
`Replay::Snapshot(&S)` (at most once, first) then `Replay::Event(&E)` for the
tail:

```rust
# extern crate es_entity;
# extern crate sqlx;
# extern crate serde;
# extern crate derive_builder;
# use serde::{Deserialize, Serialize};
# use derive_builder::Builder;
# use es_entity::*;
# es_entity::entity_id! { MeterId }
# #[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
# #[serde(tag = "type", rename_all = "snake_case")]
# #[es_event(id = "MeterId")]
# pub enum MeterEvent {
#     Initialized { id: MeterId, label: String },
#     ReadingRecorded { value: i64 },
# }
# #[derive(EsSnapshot, Debug, Clone, PartialEq, Serialize, Deserialize)]
# pub struct MeterSnapshot { pub id: MeterId, pub label: String, pub count: u32, pub total: i64 }
#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Meter {
    pub id: MeterId,
    pub label: String,
    events: EntityEvents<MeterEvent, MeterSnapshot>,
}

impl Meter {
    pub fn total(&self) -> i64 {
        self.events.replay().fold(0, |acc, r| match r {
            // The snapshot already IS the fold up to its own head — seed
            // from it, don't recompute it.
            Replay::Snapshot(s) => s.total,
            Replay::Event(MeterEvent::ReadingRecorded { value }) => acc + value,
            Replay::Event(_) => acc,
        })
    }
}

impl Snapshotting for Meter {
    fn snapshot(&self) -> Option<MeterSnapshot> {
        // Business logic decides when a snapshot is worth taking — here,
        // once four events have accumulated since the last one.
        (self.events.tail_len() >= 4).then(|| MeterSnapshot {
            id: self.id,
            label: self.label.clone(),
            count: self.total() as u32, // fold already includes the tail
            total: self.total(),
        })
    }
}
# impl TryFromEvents<MeterEvent, MeterSnapshot> for Meter {
#     fn try_from_events(events: EntityEvents<MeterEvent, MeterSnapshot>) -> Result<Self, EntityHydrationError> {
#         let mut builder = MeterBuilder::default();
#         for r in events.replay() {
#             match r {
#                 Replay::Snapshot(s) => { builder = builder.id(s.id).label(s.label.clone()); }
#                 Replay::Event(MeterEvent::Initialized { id, label }) => { builder = builder.id(*id).label(label.clone()); }
#                 Replay::Event(_) => {}
#             }
#         }
#         builder.events(events).build()
#     }
# }
# pub struct NewMeter { pub id: MeterId, pub label: String }
# impl IntoEvents<MeterEvent> for NewMeter {
#     fn into_events(self) -> EntityEvents<MeterEvent> {
#         EntityEvents::init(self.id, [MeterEvent::Initialized { id: self.id, label: self.label }])
#     }
# }
# fn main() {}
```

`Replay` is deliberately **not** `#[non_exhaustive]`: a `match` over it must
name every variant, so adding a snapshot to an entity that used to be
`NoSnapshot` turns every fold that forgot to account for `Replay::Snapshot`
into a compile error, rather than a silent under-count. `EntityEvents<E,
NoSnapshot>::iter_all()` — the pre-snapshot API — does not exist for a widened
`EntityEvents<E, S>` at all, for the same reason.

### `idempotency_guard!` on a Snapshotted Stream

A snapshot can carry the answer an idempotency guard is looking for.
`idempotency_guard!` gains a required `snapshot:` clause once its stream
yields `Replay`, so a guard that never considered the snapshot fails to
compile rather than silently missing it once the tail has been compacted
away:

```rust,ignore
idempotency_guard!(
    self.events.replay().rev(),
    already_applied: MeterEvent::ReadingRecorded { value: v } if *v == value,
    resets_on: MeterEvent::ReadingRecorded { .. },
    snapshot: s if s.last_value == Some(value),
);
```

## Repository Setup

```rust,ignore
#[derive(EsRepo, Debug)]
#[es_repo(entity = "Meter", snapshot, columns(label(ty = "String")))]
pub struct MeterRepo {
    pool: PgPool,
}
```

`repo.find_by_id`, `find_all`, `list_by_*`, `create`, `update`, and their
`_all`/`_in_op` twins all keep their existing signatures — the snapshot is
entirely a storage-layer concern.

## `full_history()`, `persist_snapshot_in_op`, and `verify_snapshot`

A snapshot repo gains three extra tools:

- **`repo.full_history()`** returns a view whose `find_by_id`/`maybe_find_by_id`
  bypass snapshots entirely (by binding a fingerprint that matches nothing),
  always replaying the complete event stream. Useful for audits, debugging,
  and as the backfill source below.
- **`repo.persist_snapshot_in_op(op, &mut entity)`** writes `snapshot()`'s
  result for an entity with no staged events (erroring if there are any),
  guarded so a concurrent writer that already moved the head wins — `Ok(true)`
  on success, `Ok(false)` if the entity was stale. This is how you backfill
  snapshots for data that predates enabling `snapshot`: load it once via
  `full_history()`, then call this to write the first snapshot.
- **`repo.verify_snapshot(id)`** is an equivalence check: it recomputes what
  the stored snapshot's fold *should* be from the full history truncated to
  the same sequence, and compares it against the stored value — `Ok(())` when
  they agree, `Err(SnapshotMismatch)` when your fold logic disagrees with
  itself depending on whether a snapshot happens to be involved (exactly the
  "forgot to seed from `Replay::Snapshot`" bug `Replay` not being
  `#[non_exhaustive]` mostly prevents, but a wildcard arm can still swallow).

## Nesting

Nested parents and children snapshot independently — any combination (both,
parent only, child only, neither) "just works": each `#[es_repo(nested)]`
child carries its own snapshot table and fingerprint, and the tree query
folds every node's snapshot join into the same single statement. See
[Nesting](./nesting.md).

## Forgettable Fields on a Snapshot

A `Forgettable<T>` field on your snapshot type is scrubbed the same way as on
an event: the repo also needs `forgettable`, and the real value is extracted
into the *existing* `<tbl>_forgettable_payloads` table, at the reserved
`sequence = 0` row (event sequences start at `1`, so `0` is always free — no
new table). `forget()` on a snapshot repo deletes that row along with the
event-level ones, deletes the stale snapshot row, and immediately re-snapshots
from the rebuilt (forgotten) state, so a snapshot never has a chance to hold a
value that should have been forgotten. See [Forgettable Data](./forgettable.md).
