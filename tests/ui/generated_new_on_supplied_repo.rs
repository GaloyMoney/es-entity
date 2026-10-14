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

pub struct NewGadget;

impl IntoEvents<GadgetEvent> for NewGadget {
    type Id = Generated;

    fn into_events(
        self,
        builder: EntityEventsBuilder<GadgetEvent, Generated>,
    ) -> EntityEvents<GadgetEvent> {
        let id = builder.id();
        builder.initial_events([GadgetEvent::Initialized { id }])
    }
}

// `NewGadget` has no id to bring; the repo expects it to.
#[derive(EsRepo)]
#[es_repo(entity = "Gadget", id = supplied)]
pub struct Gadgets {
    pool: PgPool,
}

fn main() {}
