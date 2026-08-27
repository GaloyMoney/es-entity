//! `forget()` cascading into forgettable **nested children**.
//!
//! Mirrors `tests/forgettable_nested.rs` (which covers the *delete* cascade)
//! but exercises `forget()` directly, without ever deleting anything:
//! forgetting a parent must scrub a direct child's forgettable data — delete
//! its payload rows and NULL its forgettable index columns — the same way
//! the parent's own delete cascade does, but without touching the child's
//! `deleted` flag or replaying the child's event stream.

mod helpers;

use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use es_entity::*;

es_entity::entity_id! { HouseholdId, HouseholdMemberId }

// The forgettable nested child. `name` is a forgettable event field (stored in
// the payloads table); `email` is a forgettable index column (materialised,
// nullable). Deliberately hard-delete (`delete` omitted) — cascading forget
// must not require the child to be soft-delete-capable.
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "HouseholdMemberId")]
pub enum HouseholdMemberEvent {
    Initialized {
        id: HouseholdMemberId,
        household_id: HouseholdId,
        name: Forgettable<String>,
        email: Forgettable<String>,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct HouseholdMember {
    pub id: HouseholdMemberId,
    pub household_id: HouseholdId,
    pub name: Forgettable<String>,
    pub email: Forgettable<String>,
    events: EntityEvents<HouseholdMemberEvent>,
}

impl TryFromEvents<HouseholdMemberEvent> for HouseholdMember {
    fn try_from_events(
        events: EntityEvents<HouseholdMemberEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let mut builder = HouseholdMemberBuilder::default();
        for event in events.iter_all() {
            match event {
                HouseholdMemberEvent::Initialized {
                    id,
                    household_id,
                    name,
                    email,
                } => {
                    builder = builder
                        .id(*id)
                        .household_id(*household_id)
                        .name(name.clone())
                        .email(email.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewHouseholdMember {
    pub id: HouseholdMemberId,
    pub household_id: HouseholdId,
    #[builder(setter(into))]
    pub name: String,
    #[builder(setter(into))]
    pub email: String,
}

impl NewHouseholdMember {
    pub fn builder() -> NewHouseholdMemberBuilder {
        NewHouseholdMemberBuilder::default()
    }
}

impl IntoEvents<HouseholdMemberEvent> for NewHouseholdMember {
    fn into_events(self) -> EntityEvents<HouseholdMemberEvent> {
        EntityEvents::init(
            self.id,
            [HouseholdMemberEvent::Initialized {
                id: self.id,
                household_id: self.household_id,
                name: Forgettable::new(self.name),
                email: Forgettable::new(self.email),
            }],
        )
    }
}

// The parent — also forgettable, so both "own row" and "cascade" scrub can be
// observed from a single `forget()` call.
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "HouseholdId")]
pub enum HouseholdEvent {
    Initialized {
        id: HouseholdId,
        label: Forgettable<String>,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Household {
    pub id: HouseholdId,
    pub label: Forgettable<String>,
    events: EntityEvents<HouseholdEvent>,

    #[es_entity(nested)]
    #[builder(default)]
    members: Nested<HouseholdMember>,
}

impl Household {
    pub fn add_member(&mut self, member: NewHouseholdMember) {
        self.members.add_new(member);
    }

    pub fn n_members(&self) -> usize {
        self.members.len_persisted()
    }
}

impl TryFromEvents<HouseholdEvent> for Household {
    fn try_from_events(events: EntityEvents<HouseholdEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = HouseholdBuilder::default();
        for event in events.iter_all() {
            match event {
                HouseholdEvent::Initialized { id, label } => {
                    builder = builder.id(*id).label(label.clone())
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewHousehold {
    pub id: HouseholdId,
    #[builder(setter(into))]
    pub label: String,
}

impl NewHousehold {
    pub fn builder() -> NewHouseholdBuilder {
        NewHouseholdBuilder::default()
    }
}

impl IntoEvents<HouseholdEvent> for NewHousehold {
    fn into_events(self) -> EntityEvents<HouseholdEvent> {
        EntityEvents::init(
            self.id,
            [HouseholdEvent::Initialized {
                id: self.id,
                label: Forgettable::new(self.label),
            }],
        )
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Household", forgettable, delete = "soft")]
pub struct Households {
    pool: PgPool,

    #[es_repo(nested)]
    members: HouseholdMembers,
}

impl Households {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            members: HouseholdMembers::new(pool),
        }
    }
}

// `delete = "soft"` is required here even though this suite never deletes a
// member: every nested child must satisfy `CascadeDeleteNested` for the
// parent's generated delete-cascade fn to compile, regardless of whether the
// parent (or this test) ever calls `delete()`.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "HouseholdMember",
    forgettable,
    delete = "soft",
    columns(
        household_id(ty = "HouseholdId", update(persist = false), parent),
        email(ty = "Forgettable<String>", list_by)
    )
)]
pub struct HouseholdMembers {
    pool: PgPool,
}

impl HouseholdMembers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn forget_cascades_into_forgettable_nested_children() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let households = Households::new(pool.clone());

    let household_id = HouseholdId::new();
    let member_id = HouseholdMemberId::new();
    let email = format!("member-{member_id}@example.com");

    let mut household = households
        .create(
            NewHousehold::builder()
                .id(household_id)
                .label("The Hoppers")
                .build()
                .unwrap(),
        )
        .await?;
    household.add_member(
        NewHouseholdMember::builder()
            .id(member_id)
            .household_id(household_id)
            .name("Grace Hopper")
            .email(email.clone())
            .build()
            .unwrap(),
    );
    households.update(&mut household).await?;

    // Sanity: while live, the child's index column holds the value and a
    // payload row exists for both parent and child.
    let row = sqlx::query!(
        "SELECT email FROM household_members WHERE id = $1",
        member_id as HouseholdMemberId,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(row.email.as_deref(), Some(email.as_str()));

    let member_payloads = sqlx::query!(
        "SELECT COUNT(*) AS count FROM household_members_forgettable_payloads WHERE entity_id = $1",
        member_id as HouseholdMemberId,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(member_payloads.count, Some(1));

    let household_payloads = sqlx::query!(
        "SELECT COUNT(*) AS count FROM households_forgettable_payloads WHERE entity_id = $1",
        household_id as HouseholdId,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(household_payloads.count, Some(1));

    // Forget the parent — no delete anywhere in this test.
    let household = households.find_by_id(household_id).await?;
    let forgotten = households.forget(household).await?;

    // The parent's own forgettable data is gone from the rebuilt entity...
    assert!(forgotten.label.is_forgotten());
    let household_payloads = sqlx::query!(
        "SELECT COUNT(*) AS count FROM households_forgettable_payloads WHERE entity_id = $1",
        household_id as HouseholdId,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(household_payloads.count, Some(0));

    // ...and so is the child's, even though the child was never touched
    // directly and is not soft-deleted.
    let row = sqlx::query!(
        "SELECT deleted, email FROM household_members WHERE id = $1",
        member_id as HouseholdMemberId,
    )
    .fetch_one(&pool)
    .await?;
    assert!(!row.deleted, "forget must not soft-delete the child");
    assert_eq!(row.email, None);

    let member_payloads = sqlx::query!(
        "SELECT COUNT(*) AS count FROM household_members_forgettable_payloads WHERE entity_id = $1",
        member_id as HouseholdMemberId,
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(member_payloads.count, Some(0));

    Ok(())
}
