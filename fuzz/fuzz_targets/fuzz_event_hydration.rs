#![no_main]

//! Coverage-guided fuzz target for the event-hydration path.
//!
//! `EntityEvents::load_first` / `load_n` reconstruct entities from persisted
//! event JSON. Persisted event payloads are the most untrusted data a repo
//! reads back, so we feed arbitrary JSON streams through hydration and assert
//! it never panics and respects the `load_n` limit. The harness entity is
//! deliberately non-panicking so any crash is attributable to the library.

use libfuzzer_sys::fuzz_target;

use chrono::Utc;
use es_entity::*;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Serialize, Deserialize)]
enum FuzzEvent {
    Created(String),
}

impl EsEvent for FuzzEvent {
    type EntityId = Uuid;

    fn event_context() -> bool {
        false
    }

    fn event_type(&self) -> &'static str {
        match self {
            FuzzEvent::Created(_) => "created",
        }
    }
}

struct NewFuzzEntity;

impl IntoEvents<FuzzEvent> for NewFuzzEntity {
    fn into_events(self) -> EntityEvents<FuzzEvent> {
        EntityEvents::init(Uuid::nil(), [FuzzEvent::Created("seed".to_string())])
    }
}

struct FuzzEntity {
    #[allow(dead_code)]
    name: Option<String>,
    events: EntityEvents<FuzzEvent>,
}

impl EsEntity for FuzzEntity {
    type Event = FuzzEvent;
    type New = NewFuzzEntity;

    fn events(&self) -> &EntityEvents<FuzzEvent> {
        &self.events
    }

    fn events_mut(&mut self) -> &mut EntityEvents<FuzzEvent> {
        &mut self.events
    }
}

impl TryFromEvents<FuzzEvent> for FuzzEntity {
    fn try_from_events(events: EntityEvents<FuzzEvent>) -> Result<Self, EntityHydrationError> {
        let name = events.iter_persisted().find_map(|e| match &e.event {
            FuzzEvent::Created(n) => Some(n.clone()),
        });
        Ok(FuzzEntity { name, events })
    }
}

fn make_events(values: &[serde_json::Value]) -> Vec<GenericEvent<Uuid>> {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| GenericEvent {
            entity_id: Uuid::nil(),
            sequence: i as i32,
            event: v.clone(),
            context: None,
            recorded_at: Utc::now(),
            forgettable_payload: None,
        })
        .collect()
}

fuzz_target!(|data: &[u8]| {
    let Ok(values) = serde_json::from_slice::<Vec<serde_json::Value>>(data) else {
        return;
    };
    if values.is_empty() {
        return;
    }

    let _ = EntityEvents::<FuzzEvent>::load_first::<FuzzEntity>(make_events(&values));

    let n = (values.len() % 8) + 1;
    if let Ok((entities, _more)) =
        EntityEvents::<FuzzEvent>::load_n::<FuzzEntity>(make_events(&values), n)
    {
        assert!(entities.len() <= n, "load_n exceeded the requested limit");
    }
});
