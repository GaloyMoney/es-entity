use es_entity::*;
use serde::{Deserialize, Serialize};

es_entity::entity_id! { OwnerId, ChildId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "ChildId")]
pub enum ChildEvent {
    Initialized { id: ChildId, parent_id: OwnerId },
}

#[derive(EsEntity)]
pub struct Child {
    pub id: ChildId,
    pub parent_id: OwnerId,
    events: EntityEvents<ChildEvent>,
}

impl TryFromEvents<ChildEvent> for Child {
    fn try_from_events(events: EntityEvents<ChildEvent>) -> Result<Self, EntityHydrationError> {
        let mut hydrator = Child::hydrator();
        for event in events.iter_all() {
            let ChildEvent::Initialized { id, parent_id } = event;
            hydrator.id(*id).parent_id(*parent_id);
        }
        hydrator.build(events)
    }
}

pub struct NewChild {
    pub parent_id: OwnerId,
}

impl IntoEvents<ChildEvent> for NewChild {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<ChildEvent, FromRepo>,
    ) -> EntityEvents<ChildEvent> {
        let id = builder.id();
        builder.initial_events([ChildEvent::Initialized {
            id,
            parent_id: self.parent_id,
        }])
    }
}

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "OwnerId")]
pub enum OwnerEvent {
    Initialized { id: OwnerId },
}

#[derive(EsEntity)]
pub struct Owner {
    pub id: OwnerId,
    events: EntityEvents<OwnerEvent>,

    // Plain `Nested<T>`: children cannot be deleted.
    #[es_entity(nested)]
    children: Nested<Child>,
}

impl TryFromEvents<OwnerEvent> for Owner {
    fn try_from_events(events: EntityEvents<OwnerEvent>) -> Result<Self, EntityHydrationError> {
        let mut hydrator = Owner::hydrator();
        for event in events.iter_all() {
            let OwnerEvent::Initialized { id } = event;
            hydrator.id(*id);
        }
        hydrator.build(events)
    }
}

pub struct NewOwner {
    pub id: OwnerId,
}

impl IntoEvents<OwnerEvent> for NewOwner {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<OwnerEvent, FromCaller>,
    ) -> EntityEvents<OwnerEvent> {
        builder.initial_events(self.id, [OwnerEvent::Initialized { id: self.id }])
    }
}

impl Owner {
    pub fn remove(&mut self, id: &ChildId) {
        self.children.delete_persisted(id);
    }
}

fn main() {}
