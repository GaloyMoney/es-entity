use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use es_entity::*;

es_entity::entity_id! { LaneParentId, LaneItemId }

#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "LaneItemId")]
pub enum LaneItemEvent {
    Initialized {
        id: LaneItemId,
        parent_id: LaneParentId,
        sku: String,
    },
}

#[derive(EsEntity)]
pub struct LaneItem {
    pub id: LaneItemId,
    pub parent_id: LaneParentId,
    pub sku: String,
    events: EntityEvents<LaneItemEvent>,
}

impl TryFromEvents<LaneItemEvent> for LaneItem {
    fn try_from_events(events: EntityEvents<LaneItemEvent>) -> Result<Self, EntityHydrationError> {
        let mut hydrator = LaneItem::hydrator();
        for event in events.iter_all() {
            let LaneItemEvent::Initialized { id, parent_id, sku } = event;
            hydrator.id(*id).parent_id(*parent_id).sku(sku.clone());
        }
        hydrator.build(events)
    }
}

#[derive(Debug, Clone, bon::Builder)]
pub struct NewLaneItem {
    pub id: LaneItemId,
    pub parent_id: LaneParentId,
    #[builder(into)]
    pub sku: String,
}

impl IntoEvents<LaneItemEvent> for NewLaneItem {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<LaneItemEvent, FromCaller>,
    ) -> EntityEvents<LaneItemEvent> {
        builder.initial_events(
            self.id,
            [LaneItemEvent::Initialized {
                id: self.id,
                parent_id: self.parent_id,
                sku: self.sku,
            }],
        )
    }
}

#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "LaneParentId")]
pub enum LaneParentEvent {
    Initialized { id: LaneParentId },
}

#[derive(EsEntity)]
pub struct LaneParent {
    pub id: LaneParentId,
    events: EntityEvents<LaneParentEvent>,

    #[es_entity(nested)]
    items: Nested<LaneItem>,
}

impl LaneParent {
    pub fn add_item(&mut self, item: NewLaneItem) {
        self.items.add_new(item);
    }
}

impl TryFromEvents<LaneParentEvent> for LaneParent {
    fn try_from_events(
        events: EntityEvents<LaneParentEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let mut hydrator = LaneParent::hydrator();
        for event in events.iter_all() {
            let LaneParentEvent::Initialized { id } = event;
            hydrator.id(*id);
        }
        hydrator.build(events)
    }
}

#[derive(Debug, Clone, bon::Builder)]
pub struct NewLaneParent {
    pub id: LaneParentId,
}

impl IntoEvents<LaneParentEvent> for NewLaneParent {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<LaneParentEvent, FromCaller>,
    ) -> EntityEvents<LaneParentEvent> {
        builder.initial_events(self.id, [LaneParentEvent::Initialized { id: self.id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "LaneParent", id(source = caller), delete = "soft")]
pub struct LaneParents {
    pool: PgPool,

    #[es_repo(nested)]
    items: LaneItems,
}

impl LaneParents {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            items: LaneItems::new(pool),
        }
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "LaneItem",
    id(source = caller),
    delete = "soft",
    columns(
        parent_id(ty = "LaneParentId", update(persist = false), parent),
        sku(ty = "String"),
    )
)]
pub struct LaneItems {
    pool: PgPool,
}

impl LaneItems {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}
