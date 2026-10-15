mod helpers;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use es_entity::*;

es_entity::entity_id! { PalletId, PalletItemId, RackId, RackItemId }

// ---------------------------------------------------------------------------
// Pallet: a parent on generated ids whose child has no constraint to reject on
// ---------------------------------------------------------------------------

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "PalletItemId")]
pub enum PalletItemEvent {
    Initialized {
        id: PalletItemId,
        pallet_id: PalletId,
    },
}

#[derive(EsEntity)]
pub struct PalletItem {
    pub id: PalletItemId,
    pub pallet_id: PalletId,
    events: EntityEvents<PalletItemEvent>,
}

impl TryFromEvents<PalletItemEvent> for PalletItem {
    fn try_from_events(
        events: EntityEvents<PalletItemEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let (id, pallet_id) = events
            .iter_all()
            .map(|PalletItemEvent::Initialized { id, pallet_id }| (*id, *pallet_id))
            .next()
            .expect("an initialized event");
        Ok(PalletItem {
            id,
            pallet_id,
            events,
        })
    }
}

pub struct NewPalletItem {
    pub pallet_id: PalletId,
}

impl IntoEvents<PalletItemEvent> for NewPalletItem {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<PalletItemEvent, FromRepo>,
    ) -> EntityEvents<PalletItemEvent> {
        let id = builder.id();
        builder.initial_events([PalletItemEvent::Initialized {
            id,
            pallet_id: self.pallet_id,
        }])
    }
}

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "PalletId")]
pub enum PalletEvent {
    Initialized { id: PalletId },
}

#[derive(EsEntity)]
pub struct Pallet {
    pub id: PalletId,
    events: EntityEvents<PalletEvent>,
    #[es_entity(nested)]
    items: Nested<PalletItem>,
}

impl Pallet {
    pub fn add_item(&mut self) {
        let pallet_id = self.id;
        self.items.add_new(NewPalletItem { pallet_id });
    }

    pub fn n_items(&self) -> usize {
        self.items.len_persisted()
    }
}

impl TryFromEvents<PalletEvent> for Pallet {
    fn try_from_events(events: EntityEvents<PalletEvent>) -> Result<Self, EntityHydrationError> {
        let id = events
            .iter_all()
            .map(|PalletEvent::Initialized { id }| *id)
            .next()
            .expect("an initialized event");
        Ok(Pallet {
            id,
            events,
            items: Nested::default(),
        })
    }
}

pub struct NewPallet;

impl IntoEvents<PalletEvent> for NewPallet {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<PalletEvent, FromRepo>,
    ) -> EntityEvents<PalletEvent> {
        let id = builder.id();
        builder.initial_events([PalletEvent::Initialized { id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "PalletItem",
    columns(pallet_id(ty = "PalletId", update(persist = false), parent))
)]
struct PalletItems {
    pool: PgPool,
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Pallet")]
pub struct Pallets {
    pool: PgPool,
    #[es_repo(nested)]
    items: PalletItems,
}

impl Pallets {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            items: PalletItems { pool },
        }
    }
}

// ---------------------------------------------------------------------------
// Rack: the same parent, but its child has a unique column
// ---------------------------------------------------------------------------

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "RackItemId")]
pub enum RackItemEvent {
    Initialized {
        id: RackItemId,
        rack_id: RackId,
        sku: String,
    },
}

#[derive(EsEntity)]
pub struct RackItem {
    pub id: RackItemId,
    pub rack_id: RackId,
    pub sku: String,
    events: EntityEvents<RackItemEvent>,
}

impl TryFromEvents<RackItemEvent> for RackItem {
    fn try_from_events(events: EntityEvents<RackItemEvent>) -> Result<Self, EntityHydrationError> {
        let (id, rack_id, sku) = events
            .iter_all()
            .map(|RackItemEvent::Initialized { id, rack_id, sku }| (*id, *rack_id, sku.clone()))
            .next()
            .expect("an initialized event");
        Ok(RackItem {
            id,
            rack_id,
            sku,
            events,
        })
    }
}

pub struct NewRackItem {
    pub rack_id: RackId,
    pub sku: String,
}

impl IntoEvents<RackItemEvent> for NewRackItem {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<RackItemEvent, FromRepo>,
    ) -> EntityEvents<RackItemEvent> {
        let id = builder.id();
        builder.initial_events([RackItemEvent::Initialized {
            id,
            rack_id: self.rack_id,
            sku: self.sku,
        }])
    }
}

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "RackId")]
pub enum RackEvent {
    Initialized { id: RackId },
}

#[derive(EsEntity)]
pub struct Rack {
    pub id: RackId,
    events: EntityEvents<RackEvent>,
    #[es_entity(nested)]
    items: Nested<RackItem>,
}

impl Rack {
    pub fn add_item(&mut self, sku: &str) {
        let rack_id = self.id;
        self.items.add_new(NewRackItem {
            rack_id,
            sku: sku.to_string(),
        });
    }
}

impl TryFromEvents<RackEvent> for Rack {
    fn try_from_events(events: EntityEvents<RackEvent>) -> Result<Self, EntityHydrationError> {
        let id = events
            .iter_all()
            .map(|RackEvent::Initialized { id }| *id)
            .next()
            .expect("an initialized event");
        Ok(Rack {
            id,
            events,
            items: Nested::default(),
        })
    }
}

pub struct NewRack;

impl IntoEvents<RackEvent> for NewRack {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<RackEvent, FromRepo>,
    ) -> EntityEvents<RackEvent> {
        let id = builder.id();
        builder.initial_events([RackEvent::Initialized { id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "RackItem",
    columns(
        rack_id(ty = "RackId", update(persist = false), parent),
        sku(ty = "String", update(persist = false))
    )
)]
pub struct RackItems {
    pool: PgPool,
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Rack")]
pub struct Racks {
    pool: PgPool,
    #[es_repo(nested)]
    items: RackItems,
}

impl Racks {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            items: RackItems { pool },
        }
    }
}

// ---------------------------------------------------------------------------

fn takes_fault<T>(res: Result<T, es_entity::RepoFault>) -> Result<T, es_entity::RepoFault> {
    res
}

fn takes_rack_write<T>(
    res: Result<T, es_entity::RepoWriteError<RackConstraintViolation>>,
) -> Result<T, es_entity::RepoWriteError<RackConstraintViolation>> {
    res
}

#[tokio::test]
async fn a_parent_whose_children_cannot_reject_writes_with_repo_fault() -> anyhow::Result<()> {
    let pallets = Pallets::new(helpers::init_pool().await?);

    let mut pallet = takes_fault(pallets.create(NewPallet).await)?;
    pallet.add_item();
    pallet.add_item();
    let n = takes_fault(pallets.update(&mut pallet).await)?;
    assert_eq!(n, 0, "the pallet itself has no new events");
    assert_eq!(pallet.n_items(), 2);

    let reloaded = pallets.find_by_id(pallet.id).await?;
    assert_eq!(reloaded.n_items(), 2);

    let created = takes_fault(pallets.create_all(vec![NewPallet, NewPallet]).await)?;
    assert_eq!(created.len(), 2);
    Ok(())
}

#[tokio::test]
async fn a_parent_keeps_rejecting_when_a_child_can() -> anyhow::Result<()> {
    let racks = Racks::new(helpers::init_pool().await?);
    let sku = format!("sku-{}", RackId::new());

    let mut rack = takes_rack_write(racks.create(NewRack).await)?;
    rack.add_item(&sku);
    takes_rack_write(racks.update(&mut rack).await)?;

    let mut second = takes_rack_write(racks.create(NewRack).await)?;
    second.add_item(&sku);
    let err = takes_rack_write(racks.update(&mut second).await)
        .err()
        .expect("a duplicate sku in a nested child is rejected");
    assert!(matches!(err, Fail::Rejected(_)), "got {err:?}");
    Ok(())
}
