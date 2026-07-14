//! Integration tests for `Forgettable<T>` **index columns**.
//!
//! A `columns(email = "Forgettable<String>")` column is queryable exactly like a
//! naked `String` column while the entity is live, but is transparently set to
//! NULL by `forget()` and by soft `delete()` (auto-forget) — the value survives
//! nowhere the framework materialises it.

mod helpers;

use derive_builder::Builder;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use es_entity::*;

es_entity::entity_id! { SubscriberId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "SubscriberId")]
pub enum SubscriberEvent {
    Initialized {
        id: SubscriberId,
        email: Forgettable<String>,
        plan: String,
    },
    PlanChanged {
        plan: String,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct Subscriber {
    pub id: SubscriberId,
    pub email: Forgettable<String>,
    pub plan: String,

    events: EntityEvents<SubscriberEvent>,
}

impl Subscriber {
    pub fn change_plan(&mut self, plan: impl Into<String>) -> Idempotent<()> {
        let plan = plan.into();
        self.plan = plan.clone();
        self.events.push(SubscriberEvent::PlanChanged { plan });
        Idempotent::Executed(())
    }
}

impl TryFromEvents<SubscriberEvent> for Subscriber {
    fn try_from_events(
        events: EntityEvents<SubscriberEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let mut builder = SubscriberBuilder::default();
        for event in events.iter_all() {
            match event {
                SubscriberEvent::Initialized { id, email, plan } => {
                    builder = builder.id(*id).email(email.clone()).plan(plan.clone());
                }
                SubscriberEvent::PlanChanged { plan } => {
                    builder = builder.plan(plan.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Builder)]
pub struct NewSubscriber {
    #[builder(setter(into))]
    pub id: SubscriberId,
    #[builder(setter(into))]
    pub email: String,
    #[builder(setter(into))]
    pub plan: String,
}

impl NewSubscriber {
    pub fn builder() -> NewSubscriberBuilder {
        NewSubscriberBuilder::default()
    }
}

impl IntoEvents<SubscriberEvent> for NewSubscriber {
    fn into_events(self) -> EntityEvents<SubscriberEvent> {
        EntityEvents::init(
            self.id,
            [SubscriberEvent::Initialized {
                id: self.id,
                email: Forgettable::new(self.email),
                plan: self.plan,
            }],
        )
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Subscriber",
    forgettable,
    delete = "soft",
    columns(email(ty = "Forgettable<String>", list_by), plan(ty = "String"))
)]
pub struct Subscribers {
    pool: PgPool,
}

impl Subscribers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Creates a subscriber with an email unique to this run (the table is shared
/// across tests and runs, so a fixed value would collide). Returns the entity
/// and its email.
async fn new_subscriber(subscribers: &Subscribers) -> anyhow::Result<(Subscriber, String)> {
    let id = SubscriberId::new();
    let email = format!("user-{id}@example.com");
    let new = NewSubscriber::builder()
        .id(id)
        .email(email.clone())
        .plan("pro")
        .build()
        .unwrap();
    let subscriber = subscribers.create(new).await?;
    Ok((subscriber, email))
}

#[tokio::test]
async fn forgettable_column_is_queryable_while_live() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let subscribers = Subscribers::new(pool);

    let (subscriber, email) = new_subscriber(&subscribers).await?;

    // Queryable by the inner `String`, exactly like a naked String column.
    let found = subscribers.find_by_email(&email).await?;
    assert_eq!(found.id, subscriber.id);
    assert_eq!(found.email.value().map(|v| v.clone()), Some(email));

    Ok(())
}

#[tokio::test]
async fn forget_nulls_the_index_column() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let subscribers = Subscribers::new(pool.clone());

    let (mut subscriber, email) = new_subscriber(&subscribers).await?;
    let id = subscriber.id;

    subscribers.forget(&mut subscriber).await?;

    // The rebuilt entity has forgotten the value...
    assert!(subscriber.email.is_forgotten());

    // ...the index column is now NULL in the database...
    let row = sqlx::query!(
        "SELECT email, plan FROM subscribers WHERE id = $1",
        id as SubscriberId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(row.email, None);
    assert_eq!(row.plan, "pro"); // non-forgettable column is untouched

    // ...the forgettable payload row is gone...
    let payloads = sqlx::query!(
        "SELECT COUNT(*) as count FROM subscribers_forgettable_payloads WHERE entity_id = $1",
        id as SubscriberId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(payloads.count, Some(0));

    // ...and it is no longer findable by the forgotten value.
    let refound = subscribers.maybe_find_by_email(&email).await?;
    assert!(refound.is_none());

    Ok(())
}

#[tokio::test]
async fn soft_delete_auto_forgets_the_index_column() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let subscribers = Subscribers::new(pool.clone());

    let (subscriber, _email) = new_subscriber(&subscribers).await?;
    let id = subscriber.id;

    subscribers.delete(subscriber).await?;

    // Soft-delete marks the row deleted AND nulls the forgettable column, so the
    // materialised lookup table no longer exposes the personal data.
    let row = sqlx::query!(
        "SELECT email, plan, deleted FROM subscribers WHERE id = $1",
        id as SubscriberId
    )
    .fetch_one(&pool)
    .await?;
    assert!(row.deleted);
    assert_eq!(row.email, None);
    assert_eq!(row.plan, "pro");

    Ok(())
}
