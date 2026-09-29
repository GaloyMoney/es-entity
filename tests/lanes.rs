//! R12: a nested aggregate's `{Parent}Constraint` gains one variant per
//! nested child (`{Field}({Child}Constraint)`), so `constraint()` /
//! `Liftable::key()` report a *path* into the aggregate instead of `None` for
//! a nested violation, and a domain rejection can hoist a specific nested
//! constraint by naming that path.

mod helpers;

use derive_builder::Builder;
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

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct LaneItem {
    pub id: LaneItemId,
    pub parent_id: LaneParentId,
    pub sku: String,
    events: EntityEvents<LaneItemEvent>,
}

impl TryFromEvents<LaneItemEvent> for LaneItem {
    fn try_from_events(events: EntityEvents<LaneItemEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = LaneItemBuilder::default();
        for event in events.iter_all() {
            let LaneItemEvent::Initialized { id, parent_id, sku } = event;
            builder = builder.id(*id).parent_id(*parent_id).sku(sku.clone());
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewLaneItem {
    pub id: LaneItemId,
    pub parent_id: LaneParentId,
    #[builder(setter(into))]
    pub sku: String,
}

impl NewLaneItem {
    pub fn builder() -> NewLaneItemBuilder {
        NewLaneItemBuilder::default()
    }
}

impl IntoEvents<LaneItemEvent> for NewLaneItem {
    fn into_events(self) -> EntityEvents<LaneItemEvent> {
        EntityEvents::init(
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

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct LaneParent {
    pub id: LaneParentId,
    events: EntityEvents<LaneParentEvent>,

    #[es_entity(nested)]
    #[builder(default)]
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
        let mut builder = LaneParentBuilder::default();
        for event in events.iter_all() {
            let LaneParentEvent::Initialized { id } = event;
            builder = builder.id(*id);
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewLaneParent {
    pub id: LaneParentId,
}

impl NewLaneParent {
    pub fn builder() -> NewLaneParentBuilder {
        NewLaneParentBuilder::default()
    }
}

impl IntoEvents<LaneParentEvent> for NewLaneParent {
    fn into_events(self) -> EntityEvents<LaneParentEvent> {
        EntityEvents::init(self.id, [LaneParentEvent::Initialized { id: self.id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "LaneParent", delete = "soft")]
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

/// Hoists the nested child's unique-sku violation into a domain outcome —
/// the parent key is a path: `LaneParentConstraint::Items(LaneItemConstraint::SkuKey)`.
#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
#[rejection(lift(LaneParentConstraintViolation))]
enum LaneOrderRejection {
    #[error("duplicate sku on a lane item")]
    #[rejection(key = LaneParentConstraint::Items(LaneItemConstraint::SkuKey))]
    DuplicateSku,
}

/// Same lift target, but declares no `key` at all — every nested (or Own)
/// violation demotes to `Fatal(Invariant)`, pinning the "unhoisted" case. The
/// lone variant is structurally unreachable (nothing ever matches with no
/// `key` declared) — needed only so the enum is non-empty (an empty enum's
/// generated `match &self {}` is not accepted as exhaustive).
#[allow(dead_code)]
#[derive(Debug, Clone, thiserror::Error, errlanes::Rejection)]
#[rejection(lift(LaneParentConstraintViolation))]
enum NothingHoisted {
    #[error("placeholder — never constructed")]
    Placeholder,
}

fn rejected(err: Fail<LaneParentConstraintViolation>) -> LaneParentConstraintViolation {
    match err {
        Fail::Rejected(cv) => cv,
        other => panic!("expected Rejected, got {other:?}"),
    }
}

#[tokio::test]
async fn duplicate_nested_sku_hoists_through_the_parent_path() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let parents = LaneParents::new(pool);

    let sku = format!("sku-{}", LaneParentId::new());

    let mut first = parents
        .create(
            NewLaneParent::builder()
                .id(LaneParentId::new())
                .build()
                .unwrap(),
        )
        .await?;
    first.add_item(
        NewLaneItem::builder()
            .id(LaneItemId::new())
            .parent_id(first.id)
            .sku(sku.clone())
            .build()
            .unwrap(),
    );
    parents.update(&mut first).await?;

    let mut second = parents
        .create(
            NewLaneParent::builder()
                .id(LaneParentId::new())
                .build()
                .unwrap(),
        )
        .await?;
    second.add_item(
        NewLaneItem::builder()
            .id(LaneItemId::new())
            .parent_id(second.id)
            .sku(sku.clone())
            .build()
            .unwrap(),
    );
    let err = parents
        .update(&mut second)
        .await
        .expect_err("duplicate sku across parents must be rejected");
    let cv = rejected(err);

    // The parent key is a path into the aggregate: the nested variant wraps
    // exactly the child's own key.
    assert_eq!(
        cv.constraint(),
        Some(LaneParentConstraint::Items(LaneItemConstraint::SkuKey))
    );
    // Kind-sugar and column()/value() stay parent-only — false/None for a
    // nested variant even though constraint() now reports a path.
    assert!(!cv.is_unique());
    assert_eq!(cv.column(), None);

    // A domain rejection naming that path hoists it out of Fatal.
    assert!(matches!(
        LaneOrderRejection::lift(cv.clone()),
        Ok(LaneOrderRejection::DuplicateSku)
    ));

    // A rejection that names no path at all still demotes to Fatal(Invariant),
    // whose context names the *child's* constraint.
    let fatal = NothingHoisted::lift(cv).expect_err("no key declared — must demote to Fatal");
    assert_eq!(fatal.kind, FatalKind::Invariant);
    assert!(
        fatal.context.as_deref().unwrap_or_default().contains("sku"),
        "context must name the child's own constraint, got: {:?}",
        fatal.context
    );

    Ok(())
}
