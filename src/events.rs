//! Manage events and related operations for event-sourcing.

use chrono::{DateTime, Utc};

use super::{error::EntityHydrationError, traits::*};

/// An alias for iterator over the persisted events
pub type LastPersisted<'a, E> = std::slice::Iter<'a, PersistedEvent<E>>;

/// Represent the events in raw deserialized format when loaded from database
///
/// Events in the database are stored as JSON blobs and loaded initially as `GenericEvents<Id>` where `Id`
/// belongs to the entity the events is a part of. Acts a bridge between database model and
/// domain model when later converted to the `PersistedEvent` type internally
pub struct GenericEvent<Id> {
    pub entity_id: Id,
    pub sequence: i32,
    pub event: serde_json::Value,
    pub context: Option<crate::ContextData>,
    pub recorded_at: DateTime<Utc>,
    pub forgettable_payload: Option<serde_json::Value>,
}

/// Strongly-typed event wrapper with metadata for successfully stored events.
///
/// Contains the event data along with persistence metadata (sequence, timestamp, entity_id).
/// All `new_events` from [`EntityEvents`] are converted to this structure once persisted to construct
/// entities, enabling event sourcing operations and other database operations.
pub struct PersistedEvent<E: EsEvent> {
    /// The identifier of the entity which the event is used to construct
    pub entity_id: <E as EsEvent>::EntityId,
    /// The timestamp which marks event persistence
    pub recorded_at: DateTime<Utc>,
    /// The sequence number of the event in the event stream
    pub sequence: usize,
    /// The event itself
    pub event: E,
    /// The context when the event was persisted
    /// It is only popluated if 'event_context' set on EsEvent
    pub context: Option<crate::ContextData>,
}

impl<E: Clone + EsEvent> Clone for PersistedEvent<E> {
    fn clone(&self) -> Self {
        PersistedEvent {
            entity_id: self.entity_id.clone(),
            recorded_at: self.recorded_at,
            sequence: self.sequence,
            event: self.event.clone(),
            context: self.context.clone(),
        }
    }
}

pub struct EventWithContext<E: EsEvent> {
    pub event: E,
    pub context: Option<crate::ContextData>,
}

impl<E: Clone + EsEvent> Clone for EventWithContext<E> {
    fn clone(&self) -> Self {
        EventWithContext {
            event: self.event.clone(),
            context: self.context.clone(),
        }
    }
}

/// A [`Vec`] wrapper that manages event-stream of an entity with helpers for event-sourcing operations
///
/// Provides event sourcing operations for loading, appending, and persisting events in chronological
/// sequence. Required field for all event-sourced entities to maintain their state change history.
pub struct EntityEvents<T: EsEvent> {
    /// The entity's id
    pub entity_id: <T as EsEvent>::EntityId,
    /// Events that have been persisted in database and marked
    persisted_events: Vec<PersistedEvent<T>>,
    /// New events that are yet to be persisted to track state changes
    new_events: Vec<EventWithContext<T>>,
}

impl<T: Clone + EsEvent> Clone for EntityEvents<T> {
    fn clone(&self) -> Self {
        Self {
            entity_id: self.entity_id.clone(),
            persisted_events: self.persisted_events.clone(),
            new_events: self.new_events.clone(),
        }
    }
}

impl<T> EntityEvents<T>
where
    T: EsEvent,
{
    /// Initializes a new `EntityEvents` instance with the given entity ID and initial events which is returned by [`IntoEvents`] method
    pub fn init(id: <T as EsEvent>::EntityId, initial_events: impl IntoIterator<Item = T>) -> Self {
        let context = if <T as EsEvent>::event_context() {
            Some(crate::EventContext::data_for_storing())
        } else {
            None
        };
        let new_events = initial_events
            .into_iter()
            .map(|event| EventWithContext {
                event,
                context: context.clone(),
            })
            .collect();
        Self {
            entity_id: id,
            persisted_events: Vec::new(),
            new_events,
        }
    }

    /// Returns a reference to the entity's identifier
    pub fn id(&self) -> &<T as EsEvent>::EntityId {
        &self.entity_id
    }

    /// Returns the timestamp of the first persisted event, indicating when the entity was created
    pub fn entity_first_persisted_at(&self) -> Option<DateTime<Utc>> {
        self.persisted_events.first().map(|e| e.recorded_at)
    }

    /// Returns the timestamp of the last persisted event, indicating when the entity was last modified
    pub fn entity_last_modified_at(&self) -> Option<DateTime<Utc>> {
        self.persisted_events.last().map(|e| e.recorded_at)
    }

    /// Appends a single new event to the entity's event stream to be persisted later
    pub fn push(&mut self, event: T) {
        let context = if <T as EsEvent>::event_context() {
            Some(crate::EventContext::data_for_storing())
        } else {
            None
        };
        self.new_events.push(EventWithContext { event, context });
    }

    /// Appends multiple new events to the entity's event stream to be persisted later
    pub fn extend(&mut self, events: impl IntoIterator<Item = T>) {
        let context = if <T as EsEvent>::event_context() {
            Some(crate::EventContext::data_for_storing())
        } else {
            None
        };
        self.new_events
            .extend(events.into_iter().map(|event| EventWithContext {
                event,
                context: context.clone(),
            }));
    }

    /// Returns true if there are any unpersisted events waiting to be saved
    pub fn any_new(&self) -> bool {
        !self.new_events.is_empty()
    }

    /// Returns the count of persisted events
    pub fn len_persisted(&self) -> usize {
        self.persisted_events.len()
    }

    /// Returns an iterator over all persisted events
    pub fn iter_persisted(&self) -> impl DoubleEndedIterator<Item = &PersistedEvent<T>> + Clone {
        self.persisted_events.iter()
    }

    /// Returns an iterator over the last `n` persisted events
    ///
    /// If fewer than `n` events have been persisted, all persisted events are
    /// returned instead of panicking.
    pub fn last_persisted(&self, n: usize) -> LastPersisted<'_, T> {
        let start = self.persisted_events.len().saturating_sub(n);
        self.persisted_events[start..].iter()
    }

    /// Returns an iterator over all events (both persisted and new) in chronological order
    pub fn iter_all(&self) -> impl DoubleEndedIterator<Item = &T> + Clone {
        self.persisted_events
            .iter()
            .map(|e| &e.event)
            .chain(self.new_events.iter().map(|e| &e.event))
    }

    /// Loads and reconstructs the first entity from a stream of GenericEvents, marking events as `persisted`.
    ///
    /// Returns `Ok(None)` if no events are present, `Ok(Some(entity))` on success.
    pub fn load_first<E: EsEntity<Event = T>>(
        events: impl IntoIterator<Item = GenericEvent<<T as EsEvent>::EntityId>>,
    ) -> Result<Option<E>, EntityHydrationError> {
        let mut current_id = None;
        let mut current = None;
        for e in events {
            if current_id.is_none() {
                current_id = Some(e.entity_id.clone());
                current = Some(Self {
                    entity_id: e.entity_id.clone(),
                    persisted_events: Vec::new(),
                    new_events: Vec::new(),
                });
            }
            if current_id.as_ref() != Some(&e.entity_id) {
                break;
            }
            let cur = current.as_mut().expect("Could not get current");
            let mut event_json = e.event;
            if let Some(payload) = e.forgettable_payload {
                crate::forgettable::inject_forgettable_payload(&mut event_json, payload);
            }
            cur.persisted_events.push(PersistedEvent {
                entity_id: e.entity_id,
                recorded_at: e.recorded_at,
                sequence: e.sequence as usize,
                event: serde_json::from_value(event_json)?,
                context: e.context,
            });
        }
        if let Some(current) = current {
            Ok(Some(E::try_from_events(current)?))
        } else {
            Ok(None)
        }
    }

    /// Loads and reconstructs up to `n` entities from a stream of GenericEvents.
    /// Assumes the events are grouped by `id` and ordered by `sequence` per `id`.
    ///
    /// Returns both the entities and a flag indicating whether more entities were available in the stream.
    pub fn load_n<E: EsEntity<Event = T>>(
        events: impl IntoIterator<Item = GenericEvent<<T as EsEvent>::EntityId>>,
        n: usize,
    ) -> Result<(Vec<E>, bool), EntityHydrationError> {
        if n == 0 {
            // Asking for zero entities yields zero entities. `has_more` reports
            // whether the stream was non-empty, mirroring the `LIMIT n + 1`
            // over-fetch contract the generated repos rely on: callers query
            // `LIMIT (first + 1)`, so a non-empty stream means a next page exists.
            let has_more = events.into_iter().next().is_some();
            return Ok((Vec::new(), has_more));
        }
        let mut ret: Vec<E> = Vec::new();
        let mut current_id = None;
        let mut current = None;
        for e in events {
            if current_id.as_ref() != Some(&e.entity_id) {
                if let Some(current) = current.take() {
                    ret.push(E::try_from_events(current)?);
                    if ret.len() == n {
                        return Ok((ret, true));
                    }
                }

                current_id = Some(e.entity_id.clone());
                current = Some(Self {
                    entity_id: e.entity_id.clone(),
                    persisted_events: Vec::new(),
                    new_events: Vec::new(),
                });
            }
            let cur = current.as_mut().expect("Could not get current");
            let mut event_json = e.event;
            if let Some(payload) = e.forgettable_payload {
                crate::forgettable::inject_forgettable_payload(&mut event_json, payload);
            }
            cur.persisted_events.push(PersistedEvent {
                entity_id: e.entity_id,
                recorded_at: e.recorded_at,
                sequence: e.sequence as usize,
                event: serde_json::from_value(event_json)?,
                context: e.context,
            });
        }
        if let Some(current) = current.take() {
            ret.push(E::try_from_events(current)?);
        }
        Ok((ret, false))
    }

    #[doc(hidden)]
    pub fn iter_new_events(&self) -> impl Iterator<Item = &EventWithContext<T>> {
        self.new_events.iter()
    }

    #[doc(hidden)]
    pub fn mark_new_events_persisted_at(
        &mut self,
        recorded_at: chrono::DateTime<chrono::Utc>,
    ) -> usize {
        let n = self.new_events.len();
        let offset = self.persisted_events.len() + 1;
        self.persisted_events
            .extend(
                self.new_events
                    .drain(..)
                    .enumerate()
                    .map(|(i, event)| PersistedEvent {
                        entity_id: self.entity_id.clone(),
                        recorded_at,
                        sequence: i + offset,
                        event: event.event,
                        context: event.context,
                    }),
            );
        n
    }

    #[doc(hidden)]
    pub fn new_event_types(&self) -> Vec<String> {
        self.new_events
            .iter()
            .map(|event| event.event.event_type().to_string())
            .collect()
    }

    #[doc(hidden)]
    pub fn serialize_new_events(&self) -> Vec<serde_json::Value> {
        self.new_events
            .iter()
            .map(|event| serde_json::to_value(&event.event).expect("Failed to serialize event"))
            .collect()
    }

    /// Forgets all forgettable payloads in persisted events and returns the taken events.
    ///
    /// Applies `forget_fn` to each persisted event, then takes ownership of the event
    /// stream, leaving `self` as an empty shell. The returned `EntityEvents` can be passed
    /// to `TryFromEvents::try_from_events` to rebuild the entity with forgotten fields.
    #[doc(hidden)]
    pub fn forget_and_take(&mut self, mut forget_fn: impl FnMut(&mut T)) -> Self {
        for persisted in &mut self.persisted_events {
            forget_fn(&mut persisted.event);
        }
        let entity_id = self.entity_id.clone();
        std::mem::replace(
            self,
            Self {
                entity_id,
                persisted_events: Vec::new(),
                new_events: Vec::new(),
            },
        )
    }

    #[doc(hidden)]
    pub fn serialize_new_event_contexts(&self) -> Option<Vec<crate::ContextData>> {
        if <T as EsEvent>::event_context() {
            let contexts = self
                .new_events
                .iter()
                .map(|event| event.context.clone().expect("Missing context"))
                .collect();

            Some(contexts)
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;
    use uuid::Uuid;

    /// Builds a `GenericEvent` whose JSON deserializes to `Created(name)`.
    fn valid_event(id: Uuid, sequence: i32, name: &str) -> GenericEvent<Uuid> {
        GenericEvent {
            entity_id: id,
            sequence,
            event: serde_json::to_value(DummyEntityEvent::Created(name.to_string()))
                .expect("could not serialize"),
            context: None,
            recorded_at: chrono::Utc::now(),
            forgettable_payload: None,
        }
    }

    /// Small bounded JSON strategy covering null/bool/int/float/string plus one
    /// level of array/object nesting. Cheap to shrink and enough to exercise the
    /// serde error paths without ballooning runtime.
    fn json_value() -> impl Strategy<Value = serde_json::Value> {
        let scalar = prop_oneof![
            Just(serde_json::Value::Null),
            any::<bool>().prop_map(serde_json::Value::Bool),
            any::<i64>().prop_map(serde_json::Value::from),
            any::<f64>().prop_map(serde_json::Value::from),
            ".{0,15}".prop_map(serde_json::Value::String),
        ]
        .boxed();
        let nested = prop_oneof![
            proptest::collection::vec(scalar.clone(), 0..4).prop_map(serde_json::Value::Array),
            proptest::collection::vec((".{0,6}", scalar.clone()), 0..4).prop_map(|pairs| {
                let mut m = serde_json::Map::new();
                for (k, v) in pairs {
                    m.insert(k, v);
                }
                serde_json::Value::Object(m)
            },),
        ];
        prop_oneof![scalar, nested]
    }

    #[derive(Debug, serde::Serialize, serde::Deserialize)]
    enum DummyEntityEvent {
        Created(String),
    }

    impl EsEvent for DummyEntityEvent {
        type EntityId = Uuid;
        fn event_context() -> bool {
            true
        }
        fn event_type(&self) -> &'static str {
            match self {
                Self::Created(_) => "created",
            }
        }
    }

    struct DummyEntity {
        name: String,

        events: EntityEvents<DummyEntityEvent>,
    }

    impl EsEntity for DummyEntity {
        type Event = DummyEntityEvent;
        type New = NewDummyEntity;

        fn events_mut(&mut self) -> &mut EntityEvents<DummyEntityEvent> {
            &mut self.events
        }
        fn events(&self) -> &EntityEvents<DummyEntityEvent> {
            &self.events
        }
    }

    impl TryFromEvents<DummyEntityEvent> for DummyEntity {
        fn try_from_events(
            events: EntityEvents<DummyEntityEvent>,
        ) -> Result<Self, EntityHydrationError> {
            let name = events
                .iter_persisted()
                .map(|e| match &e.event {
                    DummyEntityEvent::Created(name) => name.clone(),
                })
                .next()
                .expect("Could not find name");
            Ok(Self { name, events })
        }
    }

    struct NewDummyEntity {}

    impl IntoEvents<DummyEntityEvent> for NewDummyEntity {
        fn into_events(self) -> EntityEvents<DummyEntityEvent> {
            EntityEvents::init(
                Uuid::parse_str("00000000-0000-0000-0000-000000000000").unwrap(),
                vec![DummyEntityEvent::Created("".to_owned())],
            )
        }
    }

    #[test]
    fn load_zero_events() {
        let generic_events = vec![];
        let res = EntityEvents::load_first::<DummyEntity>(generic_events);
        assert!(matches!(res, Ok(None)));
    }

    #[test]
    fn load_first() {
        let generic_events = vec![GenericEvent {
            entity_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
            sequence: 1,
            event: serde_json::to_value(DummyEntityEvent::Created("dummy-name".to_owned()))
                .expect("Could not serialize"),
            context: None,
            recorded_at: chrono::Utc::now(),
            forgettable_payload: None,
        }];
        let entity: DummyEntity = EntityEvents::load_first(generic_events)
            .expect("Could not load")
            .expect("No entity found");
        assert!(entity.name == "dummy-name");
    }

    #[test]
    fn load_n() {
        let generic_events = vec![
            GenericEvent {
                entity_id: Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap(),
                sequence: 1,
                event: serde_json::to_value(DummyEntityEvent::Created("dummy-name".to_owned()))
                    .expect("Could not serialize"),
                context: None,
                recorded_at: chrono::Utc::now(),
                forgettable_payload: None,
            },
            GenericEvent {
                entity_id: Uuid::parse_str("00000000-0000-0000-0000-000000000003").unwrap(),
                sequence: 1,
                event: serde_json::to_value(DummyEntityEvent::Created("other-name".to_owned()))
                    .expect("Could not serialize"),
                context: None,
                recorded_at: chrono::Utc::now(),
                forgettable_payload: None,
            },
        ];
        let (entity, more): (Vec<DummyEntity>, _) =
            EntityEvents::load_n(generic_events, 2).expect("Could not load");
        assert!(!more);
        assert_eq!(entity.len(), 2);
    }

    #[test]
    fn last_persisted_does_not_panic_when_n_exceeds_len() {
        let generic_events = vec![GenericEvent {
            entity_id: Uuid::parse_str("00000000-0000-0000-0000-000000000004").unwrap(),
            sequence: 1,
            event: serde_json::to_value(DummyEntityEvent::Created("dummy".to_owned()))
                .expect("Could not serialize"),
            context: None,
            recorded_at: chrono::Utc::now(),
            forgettable_payload: None,
        }];
        let entity: DummyEntity = EntityEvents::load_first(generic_events)
            .expect("Could not load")
            .expect("No entity found");
        let events = entity.events();

        // n == len works
        assert_eq!(events.last_persisted(1).count(), 1);
        // n > len clamps to all persisted events instead of underflowing
        assert_eq!(events.last_persisted(10).count(), 1);
        // n == 0 yields nothing
        assert_eq!(events.last_persisted(0).count(), 0);
    }

    proptest! {
        #[test]
        fn load_first_empty_input_returns_none(_ in Just(())) {
            let res = EntityEvents::<DummyEntityEvent>::load_first::<DummyEntity>(vec![]);
            prop_assert!(matches!(res, Ok(None)));
        }

        #[test]
        fn load_first_non_empty_valid_hydrates(
            count in 1u8..8,
            names in proptest::collection::vec(".{0,10}", 1..8),
        ) {
            let id = Uuid::nil();
            let events: Vec<_> = (1..=count as i32)
                .zip(names.iter())
                .map(|(s, n)| valid_event(id, s, n))
                .collect();
            let res = EntityEvents::<DummyEntityEvent>::load_first::<DummyEntity>(events);
            prop_assert!(matches!(res, Ok(Some(_))));
        }

        /// Feeds arbitrary JSON through the loader. It may hydrate or error, but
        /// it must never panic — and `Ok(None)` is only possible for empty input.
        #[test]
        fn load_first_arbitrary_json_never_panics(
            raw in proptest::collection::vec(json_value(), 0..10),
        ) {
            let events: Vec<_> = raw
                .into_iter()
                .enumerate()
                .map(|(i, v)| GenericEvent {
                    entity_id: Uuid::nil(),
                    sequence: i as i32,
                    event: v,
                    context: None,
                    recorded_at: chrono::Utc::now(),
                    forgettable_payload: None,
                })
                .collect();
            let is_empty = events.is_empty();
            let res = EntityEvents::<DummyEntityEvent>::load_first::<DummyEntity>(events);
            if let Ok(None) = res {
                prop_assert!(is_empty);
            }
        }

        /// With a grouped, ascending stream (the documented precondition) and
        /// `n >= 1`, `load_n` returns `min(n, k)` entities and reports `has_more`
        /// exactly when `n < k`.
        #[test]
        fn load_n_respects_limit_and_more_flag(
            k in 1u8..8,
            per in 1u8..4,
            n in 1u8..12,
        ) {
            let mut events = Vec::new();
            for i in 0..k {
                let id = Uuid::from_u128(i as u128);
                for s in 1..=per as i32 {
                    events.push(valid_event(id, s, &format!("e{i}-{s}")));
                }
            }
            let (entities, has_more) =
                EntityEvents::<DummyEntityEvent>::load_n::<DummyEntity>(events, n as usize)
                    .expect("valid events hydrate");
            prop_assert_eq!(entities.len(), (n as usize).min(k as usize));
            prop_assert_eq!(has_more, n < k);
        }

        /// Regression for the `n = 0` degenerate case: previously returned *all*
        /// entities instead of zero. Now returns none and reports `has_more` iff
        /// the stream was non-empty (the `LIMIT n + 1` contract).
        #[test]
        fn load_n_zero_returns_no_entities(k in 0u8..5, per in 1u8..3) {
            let mut events = Vec::new();
            for i in 0..k {
                let id = Uuid::from_u128(i as u128);
                for s in 1..=per as i32 {
                    events.push(valid_event(id, s, &format!("e{i}-{s}")));
                }
            }
            let (entities, has_more) =
                EntityEvents::<DummyEntityEvent>::load_n::<DummyEntity>(events, 0)
                    .expect("valid events hydrate");
            prop_assert!(entities.is_empty());
            prop_assert_eq!(has_more, k > 0);
        }

        #[test]
        fn load_n_arbitrary_json_never_panics(
            raw in proptest::collection::vec(json_value(), 0..10),
            n in 0u8..12,
        ) {
            let events: Vec<_> = raw
                .into_iter()
                .enumerate()
                .map(|(i, v)| GenericEvent {
                    entity_id: Uuid::nil(),
                    sequence: i as i32,
                    event: v,
                    context: None,
                    recorded_at: chrono::Utc::now(),
                    forgettable_payload: None,
                })
                .collect();
            let _ = EntityEvents::<DummyEntityEvent>::load_n::<DummyEntity>(events, n as usize);
        }

        /// `last_persisted(n)` must clamp to the available events for any `n`,
        /// generalizing the dedicated saturating-sub test above.
        #[test]
        fn last_persisted_clamps_for_any_n(
            p in 1u8..8,
            n in 0u16..12,
        ) {
            let id = Uuid::nil();
            let events: Vec<_> = (1..=p as i32)
                .map(|s| valid_event(id, s, &format!("n{s}")))
                .collect();
            let entity: DummyEntity =
                EntityEvents::<DummyEntityEvent>::load_first::<DummyEntity>(events)
                    .expect("load")
                    .expect("some");
            let count = entity.events().last_persisted(n as usize).count();
            prop_assert_eq!(count, (n as usize).min(p as usize));
        }

        /// Marking new events as persisted must drain `new_events`, leave
        /// `len_persisted` consistent, and assign contiguous 1-based sequences
        /// across repeated calls.
        #[test]
        fn mark_new_events_assigns_contiguous_sequences(
            a in 0u8..5,
            b in 0u8..5,
        ) {
            let id = Uuid::nil();
            let mut events = EntityEvents::init(
                id,
                (0..a).map(|i| DummyEntityEvent::Created(format!("n{i}"))),
            );
            let now = chrono::Utc::now();

            prop_assert_eq!(events.mark_new_events_persisted_at(now), a as usize);
            prop_assert!(!events.any_new());
            prop_assert_eq!(events.len_persisted(), a as usize);
            let seqs: Vec<usize> = events.iter_persisted().map(|e| e.sequence).collect();
            prop_assert_eq!(seqs, (1..=a as usize).collect::<Vec<_>>());

            for i in 0..b {
                events.push(DummyEntityEvent::Created(format!("m{i}")));
            }
            if b > 0 {
                prop_assert!(events.any_new());
            }
            prop_assert_eq!(events.mark_new_events_persisted_at(now), b as usize);
            prop_assert!(!events.any_new());
            prop_assert_eq!(events.len_persisted(), (a + b) as usize);
            let seqs: Vec<usize> = events.iter_persisted().map(|e| e.sequence).collect();
            prop_assert_eq!(seqs, (1..=(a + b) as usize).collect::<Vec<_>>());
        }
    }
}
