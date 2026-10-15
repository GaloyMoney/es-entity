use es_entity::{testing::*, *};
use serde::{Deserialize, Serialize};

entity_id! { GadgetId, WidgetId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "GadgetId")]
pub enum GadgetEvent {
    Initialized { id: GadgetId, name: String },
    Renamed { name: String },
}

#[derive(EsEntity)]
pub struct Gadget {
    pub id: GadgetId,
    pub name: String,
    events: EntityEvents<GadgetEvent>,
}

impl Gadget {
    pub fn rename(&mut self, name: &str) {
        self.name = name.to_string();
        self.events.push(GadgetEvent::Renamed {
            name: self.name.clone(),
        });
    }
}

impl TryFromEvents<GadgetEvent> for Gadget {
    fn try_from_events(events: EntityEvents<GadgetEvent>) -> Result<Self, EntityHydrationError> {
        let mut id = None;
        let mut name = String::new();
        for event in events.iter_all() {
            match event {
                GadgetEvent::Initialized { id: i, name: n } => {
                    id = Some(*i);
                    name = n.clone();
                }
                GadgetEvent::Renamed { name: n } => name = n.clone(),
            }
        }
        Ok(Gadget {
            id: id.ok_or(EntityHydrationError::UninitializedFieldError(
                derive_builder::UninitializedFieldError::new("id"),
            ))?,
            name,
            events,
        })
    }
}

pub struct NewGadget {
    name: String,
}

impl IntoEvents<GadgetEvent> for NewGadget {
    type Id = Generated;

    fn into_events(
        self,
        builder: EntityEventsBuilder<GadgetEvent, Generated>,
    ) -> EntityEvents<GadgetEvent> {
        let id = builder.id();
        builder.initial_events([GadgetEvent::Initialized {
            id,
            name: self.name,
        }])
    }
}

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "WidgetId")]
pub enum WidgetEvent {
    Initialized { id: WidgetId },
}

#[derive(EsEntity)]
pub struct Widget {
    pub id: WidgetId,
    events: EntityEvents<WidgetEvent>,
}

impl TryFromEvents<WidgetEvent> for Widget {
    fn try_from_events(events: EntityEvents<WidgetEvent>) -> Result<Self, EntityHydrationError> {
        Ok(Widget {
            id: *events.id(),
            events,
        })
    }
}

pub struct NewWidget {
    id: WidgetId,
}

impl IntoEvents<WidgetEvent> for NewWidget {
    type Id = Supplied;

    fn into_events(
        self,
        builder: EntityEventsBuilder<WidgetEvent, Supplied>,
    ) -> EntityEvents<WidgetEvent> {
        builder.initial_events(self.id, [WidgetEvent::Initialized { id: self.id }])
    }
}

#[test]
fn hydrate_from_events_replays_an_event_stream_as_persisted() {
    let id = GadgetId::new();
    let gadget = Gadget::hydrate_from_events(
        id,
        [
            GadgetEvent::Initialized {
                id,
                name: "a".to_string(),
            },
            GadgetEvent::Renamed {
                name: "b".to_string(),
            },
        ],
    )
    .unwrap();
    assert_eq!(gadget.id, id);
    assert_eq!(gadget.name, "b");
    assert_eq!(gadget.events().len_persisted(), 2);
    assert_eq!(gadget.events().len_new(), 0);
    assert_eq!(
        gadget
            .events()
            .iter_persisted()
            .map(|e| e.sequence)
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn hydrate_from_events_rejects_a_payload_id_that_is_not_the_stream_id() {
    let result = Gadget::hydrate_from_events(
        GadgetId::new(),
        [GadgetEvent::Initialized {
            id: GadgetId::new(),
            name: "a".to_string(),
        }],
    );
    assert!(matches!(result, Err(EntityHydrationError::IdMismatch)));
}

#[test]
fn hydrate_from_new_takes_the_id_when_the_repo_generates_it() {
    let id = GadgetId::new();
    let mut gadget = Gadget::hydrate_from_new(
        id,
        NewGadget {
            name: "a".to_string(),
        },
    )
    .unwrap();
    assert_eq!(gadget.id, id);
    assert_eq!(gadget.name, "a");

    assert_eq!(gadget.events().len_persisted(), 1);
    assert_eq!(gadget.events().len_new(), 0);

    gadget.rename("b");
    assert_eq!(gadget.events().len_persisted(), 1);
    assert_eq!(gadget.events().len_new(), 1);
}

#[test]
fn hydrate_from_new_takes_only_the_new_value_when_it_supplies_the_id() {
    let id = WidgetId::new();
    let widget = Widget::hydrate_from_new(NewWidget { id }).unwrap();
    assert_eq!(widget.id, id);
    assert_eq!(widget.events().len_persisted(), 1);
    assert_eq!(widget.events().len_new(), 0);
}
