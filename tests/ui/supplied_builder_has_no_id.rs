use es_entity::*;
use serde::{Deserialize, Serialize};

es_entity::entity_id! { GadgetId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "GadgetId")]
pub enum GadgetEvent {
    Initialized { id: GadgetId },
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
        let id = builder.id();
        builder.initial_events(id, [GadgetEvent::Initialized { id }])
    }
}

fn main() {}
