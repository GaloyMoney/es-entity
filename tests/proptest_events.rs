//! Property-based tests for `EntityEvents` invariants.
//!
//! Targets in `src/events.rs`:
//!   - `mark_new_events_persisted_at` sequencing
//!   - `load_first` / `load_n` JSON roundtrip
//!   - `load_n` grouping across multiple entity ids
//!   - `iter_all` ordering (persisted then new, in insertion order)

use chrono::{TimeZone, Utc};
use es_entity::*;
use proptest::collection::vec;
use proptest::prelude::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum TestEvent {
    Created { id: Uuid, name: String },
    Renamed { name: String },
    Tagged { tag: String },
}

impl EsEvent for TestEvent {
    type EntityId = Uuid;
    fn event_context() -> bool {
        false
    }
    fn event_type(&self) -> &'static str {
        match self {
            TestEvent::Created { .. } => "created",
            TestEvent::Renamed { .. } => "renamed",
            TestEvent::Tagged { .. } => "tagged",
        }
    }
}

struct TestEntity {
    events: EntityEvents<TestEvent>,
}

impl EsEntity for TestEntity {
    type Event = TestEvent;
    type New = NewTestEntity;
    fn events(&self) -> &EntityEvents<TestEvent> {
        &self.events
    }
    fn events_mut(&mut self) -> &mut EntityEvents<TestEvent> {
        &mut self.events
    }
}

impl TryFromEvents<TestEvent> for TestEntity {
    fn try_from_events(events: EntityEvents<TestEvent>) -> Result<Self, EntityHydrationError> {
        Ok(Self { events })
    }
}

struct NewTestEntity {
    id: Uuid,
}

impl IntoEvents<TestEvent> for NewTestEntity {
    fn into_events(self) -> EntityEvents<TestEvent> {
        EntityEvents::init(
            self.id,
            [TestEvent::Created {
                id: self.id,
                name: String::new(),
            }],
        )
    }
}

// ---------- generators ----------

fn arb_event(id: Uuid) -> impl Strategy<Value = TestEvent> {
    prop_oneof![
        ".*".prop_map(move |name| TestEvent::Created { id, name }),
        ".*".prop_map(|name| TestEvent::Renamed { name }),
        ".*".prop_map(|tag| TestEvent::Tagged { tag }),
    ]
}

fn arb_event_seq(id: Uuid, max_len: usize) -> impl Strategy<Value = Vec<TestEvent>> {
    vec(arb_event(id), 1..=max_len)
}

/// "Steps": either push k events, or mark all currently buffered as persisted.
#[derive(Debug, Clone)]
enum Step {
    Push(Vec<TestEvent>),
    MarkPersisted,
}

fn arb_steps(id: Uuid) -> impl Strategy<Value = Vec<Step>> {
    let step = prop_oneof![
        arb_event_seq(id, 5).prop_map(Step::Push),
        Just(Step::MarkPersisted),
    ];
    vec(step, 1..15)
}

// ---------- properties ----------

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, ..ProptestConfig::default() })]

    /// After every `mark_new_events_persisted_at` call, persisted events have:
    ///   - contiguous sequences starting at 1
    ///   - monotonically increasing sequences
    ///   - new_events buffer drained
    /// Across arbitrary interleavings of push/extend/mark.
    #[test]
    fn mark_persisted_sequencing(steps in arb_steps(Uuid::nil())) {
        let id = Uuid::nil();
        let mut events: EntityEvents<TestEvent> = EntityEvents::init(id, std::iter::empty());
        let mut t = 0i64;

        for step in steps {
            match step {
                Step::Push(evs) => events.extend(evs),
                Step::MarkPersisted => {
                    if events.any_new() {
                        t += 1;
                        events.mark_new_events_persisted_at(Utc.timestamp_opt(t, 0).unwrap());
                    }
                    prop_assert!(!events.any_new(), "buffer should be empty after mark");

                    let seqs: Vec<usize> = events.iter_persisted().map(|e| e.sequence).collect();
                    for (i, s) in seqs.iter().enumerate() {
                        prop_assert_eq!(*s, i + 1, "sequences must be contiguous from 1");
                    }
                }
            }
        }
    }

    /// Roundtrip: events serialized through GenericEvent and back via load_first
    /// reproduce the original event sequence in order.
    #[test]
    fn load_first_roundtrip(evs in arb_event_seq(Uuid::from_u128(1), 20)) {
        let id = Uuid::from_u128(1);
        let generic: Vec<GenericEvent<Uuid>> = evs
            .iter()
            .enumerate()
            .map(|(i, e)| GenericEvent {
                entity_id: id,
                sequence: (i + 1) as i32,
                event: serde_json::to_value(e).unwrap(),
                context: None,
                recorded_at: Utc.timestamp_opt(1_700_000_000 + i as i64, 0).unwrap(),
            })
            .collect();

        let entity: TestEntity = EntityEvents::load_first(generic)
            .expect("load_first")
            .expect("entity present");

        let loaded: Vec<&TestEvent> = entity.events().iter_all().collect();
        prop_assert_eq!(loaded.len(), evs.len());
        for (a, b) in loaded.iter().zip(evs.iter()) {
            prop_assert_eq!(*a, b);
        }

        // sequences are preserved 1..=N
        let seqs: Vec<usize> = entity.events().iter_persisted().map(|e| e.sequence).collect();
        prop_assert_eq!(seqs, (1..=evs.len()).collect::<Vec<_>>());
    }

    /// `load_n` correctly groups events by entity_id and respects the `n` cap.
    /// Generates K entities each with M events; all events for a given id appear
    /// contiguously and in sequence order (the documented input contract).
    #[test]
    fn load_n_grouping(
        per_entity in vec(1usize..6, 1..8),
        n in 1usize..10,
    ) {
        let total_entities = per_entity.len();
        let mut generic: Vec<GenericEvent<Uuid>> = Vec::new();
        let mut t = 1_700_000_000i64;

        for (idx, count) in per_entity.iter().enumerate() {
            let id = Uuid::from_u128(idx as u128 + 1);
            for seq in 1..=*count {
                generic.push(GenericEvent {
                    entity_id: id,
                    sequence: seq as i32,
                    event: serde_json::to_value(TestEvent::Tagged {
                        tag: format!("e{idx}-s{seq}"),
                    })
                    .unwrap(),
                    context: None,
                    recorded_at: Utc.timestamp_opt(t, 0).unwrap(),
                });
                t += 1;
            }
        }

        let total_events = generic.len();
        let (entities, more) =
            EntityEvents::<TestEvent>::load_n::<TestEntity>(generic, n).expect("load_n");

        let expected_returned = total_entities.min(n);
        prop_assert_eq!(entities.len(), expected_returned);
        prop_assert_eq!(more, total_entities > n);

        // Each returned entity contains the right number of events for its position.
        for (i, ent) in entities.iter().enumerate() {
            prop_assert_eq!(ent.events().len_persisted(), per_entity[i]);
            // and the entity_id matches what we generated
            prop_assert_eq!(*ent.events().id(), Uuid::from_u128(i as u128 + 1));
        }

        // No event lost when n >= total_entities.
        if !more {
            let summed: usize = entities.iter().map(|e| e.events().len_persisted()).sum();
            prop_assert_eq!(summed, total_events);
        }
    }

    /// `iter_all` yields persisted events first (in sequence order), then new events
    /// in push order. Holds across any interleaving of push and mark.
    #[test]
    fn iter_all_ordering(steps in arb_steps(Uuid::nil())) {
        let id = Uuid::nil();
        let mut events: EntityEvents<TestEvent> = EntityEvents::init(id, std::iter::empty());
        let mut expected: Vec<TestEvent> = Vec::new();
        let mut t = 0i64;

        for step in steps {
            match step {
                Step::Push(evs) => {
                    expected.extend(evs.iter().cloned());
                    events.extend(evs);
                }
                Step::MarkPersisted => {
                    if events.any_new() {
                        t += 1;
                        events.mark_new_events_persisted_at(Utc.timestamp_opt(t, 0).unwrap());
                    }
                }
            }
            // Invariant after every step: iter_all yields exactly `expected`.
            let actual: Vec<TestEvent> = events.iter_all().cloned().collect();
            prop_assert_eq!(&actual, &expected);
        }
    }
}
