use es_entity::*;
use serde::{Deserialize, Serialize};

es_entity::entity_id! { GadgetId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "GadgetId")]
pub enum GadgetEvent {
    Initialized { id: GadgetId },
}

fn main() {
    let _builder = EntityEventsBuilder::<GadgetEvent, FromRepo> {
        id: GadgetId::new(),
    };
}
