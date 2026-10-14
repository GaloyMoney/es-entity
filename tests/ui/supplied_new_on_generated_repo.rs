use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! { GadgetId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "GadgetId")]
pub enum GadgetEvent {
    Initialized { id: GadgetId },
}

#[derive(EsEntity)]
pub struct Gadget {
    pub id: GadgetId,
    events: EntityEvents<GadgetEvent>,
}

impl TryFromEvents<GadgetEvent> for Gadget {
    fn try_from_events(events: EntityEvents<GadgetEvent>) -> Result<Self, EntityHydrationError> {
        Ok(Gadget {
            id: *events.id(),
            events,
        })
    }
}

pub struct NewGadget {
    pub id: GadgetId,
}

impl IntoEvents<GadgetEvent> for NewGadget {
    type Id = Supplied;

    fn into_events(
        self,
        builder: EntityEventsBuilder<GadgetEvent, Supplied>,
    ) -> EntityEvents<GadgetEvent> {
        builder.initial_events(self.id, [GadgetEvent::Initialized { id: self.id }])
    }
}

// The default policy generates the id; `NewGadget` brings its own.
#[derive(EsRepo)]
#[es_repo(entity = "Gadget")]
pub struct Gadgets {
    pool: PgPool,
}

fn main() {}
