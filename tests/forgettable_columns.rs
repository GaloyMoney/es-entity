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
    EmailUpdated {
        email: Forgettable<String>,
    },
    /// Client-declared erasure event (convention): stage before `forget()`
    /// or `delete()` so the erasure consumes a sequence number.
    Forgot {},
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

    pub fn update_email(&mut self, email: impl Into<String>) -> Idempotent<()> {
        let email = email.into();
        self.email = Forgettable::new(email.clone());
        self.events.push(SubscriberEvent::EmailUpdated {
            email: Forgettable::new(email),
        });
        Idempotent::Executed(())
    }

    /// Stages the domain erasure event (convention — see the book chapter).
    pub fn record_erasure(&mut self) {
        self.events.push(SubscriberEvent::Forgot {});
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
                SubscriberEvent::EmailUpdated { email } => {
                    builder = builder.email(email.clone());
                }
                // Erasure event (client convention): payloads were deleted
                // at this point in the stream. No state to apply.
                SubscriberEvent::Forgot { .. } => {}
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

    let (subscriber, email) = new_subscriber(&subscribers).await?;
    let id = subscriber.id;

    let subscriber = subscribers.forget(subscriber).await?;

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
async fn delete_with_pending_forgettable_events_leaves_the_pending_payload() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let subscribers = Subscribers::new(pool.clone());

    let (mut subscriber, _email) = new_subscriber(&subscribers).await?;
    let id = subscriber.id;

    // Base soft-delete ordering: it deletes the payload rows first, then
    // persists any pending events. A pending forgettable-carrying event
    // therefore inserts its payload row *after* the delete, so that row
    // survives the soft delete — the accepted base behavior. (Staging the
    // erasure event, not smuggling forgettable data through an unpersisted
    // event, is the convention for actually forgetting.)
    let _ = subscriber.update_email(format!("pending-{id}@example.com"));
    subscribers.delete(subscriber).await?;

    let payloads = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!" FROM subscribers_forgettable_payloads
           WHERE entity_id = $1"#,
        id as SubscriberId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        payloads.count, 1,
        "the pending event's payload survives base soft-delete ordering"
    );

    // The pending event was persisted, and its durable JSON holds null (the
    // value lives in the surviving payload row).
    let row = sqlx::query!(
        r#"SELECT event->>'email' IS NULL AS "email_is_null!" FROM subscriber_events
           WHERE id = $1 AND event_type = 'email_updated'"#,
        id as SubscriberId
    )
    .fetch_one(&pool)
    .await?;
    assert!(row.email_is_null);

    Ok(())
}

#[tokio::test]
async fn staged_erasure_event_fences_stale_writers_on_delete() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let subscribers = Subscribers::new(pool.clone());

    let (mut subscriber, email) = new_subscriber(&subscribers).await?;
    let id = subscriber.id;

    // A stale copy loaded before the delete.
    let mut stale = subscribers.find_by_id(id).await?;

    // Convention: stage the erasure event before deleting. delete() persists
    // it as part of the soft delete, consuming a sequence number — the
    // concurrency fence (independent of the payload-delete ordering). Without
    // a staged event the delete consumes no sequence and a stale writer stays
    // unfenced (accepted tradeoff, see the book chapter).
    subscriber.record_erasure();
    subscribers.delete(subscriber).await?;

    let _ = stale.change_plan("basic");
    let err = subscribers
        .update(&mut stale)
        .await
        .expect_err("stale update after fenced delete must fail");
    assert!(
        err.was_concurrent_modification(),
        "expected ConcurrentModification, got: {err}"
    );

    // The index column stays NULL.
    let row = sqlx::query!(
        "SELECT email, deleted FROM subscribers WHERE id = $1",
        id as SubscriberId
    )
    .fetch_one(&pool)
    .await?;
    assert!(row.deleted);
    assert_eq!(row.email, None);
    // Silence unused warning; the forgotten value must not be findable anyway.
    let _ = email;

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
