//! `EntityEvents::created_at` is the total form of
//! `entity_first_persisted_at` for an entity a repository persisted. The
//! container is the unit under test, so it is built through the builder the
//! derive uses: `hydrate_from_*` hands back an entity, not its events.

use chrono::{DateTime, TimeZone as _, Utc};
use es_entity::{EntityEvents, EntityEventsBuilder, EsSnapshot, entity_id};
use serde::{Deserialize, Serialize};

entity_id! { MeterId }

#[derive(es_entity::EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "MeterId")]
enum MeterEvent {
    Initialized { id: MeterId },
    Touched,
}

#[derive(EsSnapshot, Debug, Clone, Serialize, Deserialize)]
#[es_snapshot(version = 1)]
struct MeterSnapshot {
    touches: usize,
}

fn at(second: u32) -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, second)
        .single()
        .expect("a valid timestamp")
}

fn initialized(id: MeterId) -> EntityEvents<MeterEvent> {
    EntityEventsBuilder::__from_repo(id).initial_events([MeterEvent::Initialized { id }])
}

#[test]
fn a_persisted_stream_answers_with_the_first_persisted_event() {
    let id = MeterId::new();
    let first = at(1);
    let second = at(2);

    let mut events = initialized(id);
    events.mark_new_events_persisted_at(first);
    events.push(MeterEvent::Touched);
    events.mark_new_events_persisted_at(second);

    assert_eq!(events.created_at(), first);
    assert_eq!(events.entity_first_persisted_at(), Some(first));
    assert_eq!(events.entity_last_modified_at(), Some(second));
}

#[test]
fn a_snapshot_only_stream_answers_with_the_snapshots_first_recorded_at() {
    let id = MeterId::new();
    let first = at(1);
    let snapshot_recorded_at = at(2);
    let tail = at(3);

    let mut events = initialized(id);
    events.mark_new_events_persisted_at(first);
    let mut events = events.widen_snapshot::<MeterSnapshot>();
    events.compact_to_snapshot(MeterSnapshot { touches: 1 }, snapshot_recorded_at, first);
    events.push(MeterEvent::Touched);
    events.mark_new_events_persisted_at(tail);

    assert_eq!(events.created_at(), first);
    assert_eq!(events.entity_first_persisted_at(), Some(first));
    assert_eq!(events.entity_last_modified_at(), Some(tail));
}

#[test]
fn a_creation_only_stream_answers_with_the_events_recorded_at() {
    let first = at(1);

    let mut events = initialized(MeterId::new());
    events.mark_new_events_persisted_at(first);

    assert_eq!(events.created_at(), first);
}

#[test]
#[should_panic(expected = "created_at is defined only for a persisted entity")]
fn an_unpersisted_stream_panics() {
    let _ = initialized(MeterId::new()).created_at();
}
