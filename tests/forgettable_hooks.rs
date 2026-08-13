//! Post-persist hook execution on the generated `forget` / `forget_in_op`
//! path.
//!
//! Forgetting an entity must run the same post-persist hook as normal
//! persists — exactly once, after the payload delete and rebuild (so the hook
//! observes the forgotten representation), and inside the erasure transaction
//! (so a failing hook rolls the whole erasure back and outbox-style
//! publishers never publish uncommitted data).

mod entities;
mod helpers;

use sqlx::PgPool;

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use entities::customer::*;
use es_entity::*;

#[derive(Debug)]
pub struct CustomerPublishError(String);

impl std::fmt::Display for CustomerPublishError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "publish failed: {}", self.0)
    }
}

impl std::error::Error for CustomerPublishError {}

/// One recorded hook invocation.
#[derive(Debug, Clone)]
pub struct HookCall {
    /// Event types of the events handed to the hook, in order.
    pub event_types: Vec<String>,
    /// The entity's `name` as the hook observed it.
    pub entity_name: String,
    /// Whether any `NameUpdated` event handed to the hook still carried a
    /// readable (non-forgotten) name value.
    pub saw_raw_name: bool,
    /// Payload rows present for the entity at hook time (queried through the
    /// hook's own operation — i.e. inside the same transaction).
    pub payload_rows_at_hook_time: i64,
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Customer",
    forgettable,
    columns(email(ty = "String")),
    post_persist_hook(method = "publish", error = "CustomerPublishError")
)]
pub struct CustomersWithHook {
    pool: PgPool,
    calls: Mutex<Vec<HookCall>>,
    fail_on_forgot: AtomicBool,
}

impl CustomersWithHook {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            calls: Mutex::new(Vec::new()),
            fail_on_forgot: AtomicBool::new(false),
        }
    }

    pub fn calls(&self) -> Vec<HookCall> {
        self.calls.lock().unwrap().clone()
    }

    async fn publish<OP: es_entity::AtomicOperation>(
        &self,
        op: &mut OP,
        entity: &Customer,
        new_events: es_entity::events::LastPersisted<'_, CustomerEvent>,
    ) -> Result<(), CustomerPublishError> {
        let events: Vec<_> = new_events.collect();
        let event_types: Vec<String> = events
            .iter()
            .map(|e| match &e.event {
                CustomerEvent::Initialized { .. } => "initialized".to_string(),
                CustomerEvent::NameUpdated { .. } => "name_updated".to_string(),
                CustomerEvent::EmailUpdated { .. } => "email_updated".to_string(),
                CustomerEvent::Forgot { .. } => "forgot".to_string(),
            })
            .collect();
        let saw_raw_name = events.iter().any(|e| match &e.event {
            CustomerEvent::NameUpdated { name } => name.value().is_some(),
            _ => false,
        });
        let id = entity.id;
        let payload_rows_at_hook_time = sqlx::query!(
            r#"SELECT COUNT(*) AS "count!" FROM customers_forgettable_payloads
               WHERE entity_id = $1"#,
            id as CustomerId
        )
        .fetch_one(op.as_executor())
        .await
        .map_err(|e| CustomerPublishError(e.to_string()))?
        .count;

        if self.fail_on_forgot.load(Ordering::SeqCst) && event_types.iter().any(|t| t == "forgot") {
            return Err(CustomerPublishError(format!(
                "publisher rejected forgot event for {id}"
            )));
        }

        self.calls.lock().unwrap().push(HookCall {
            event_types,
            entity_name: entity.name.clone(),
            saw_raw_name,
            payload_rows_at_hook_time,
        });
        Ok(())
    }
}

#[tokio::test]
async fn forget_runs_post_persist_hook_for_staged_erasure_event() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = CustomersWithHook::new(pool);

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Hooked Hannah")
        .email("hooked@example.com")
        .build()
        .unwrap();
    let mut customer = customers.create(new_customer).await?;
    assert_eq!(customers.calls().len(), 1, "create fires the hook once");

    customer.record_erasure();
    customers.forget(customer).await?;

    let calls = customers.calls();
    assert_eq!(calls.len(), 2, "forget fires the hook exactly once more");
    let forget_call = &calls[1];
    assert_eq!(forget_call.event_types, vec!["forgot"]);
    // The hook observes the rebuilt (forgotten) entity, not the raw one.
    assert_eq!(forget_call.entity_name, "[forgotten]");
    // ...and runs after the payload delete, inside the erasure transaction.
    assert_eq!(forget_call.payload_rows_at_hook_time, 0);

    Ok(())
}

#[tokio::test]
async fn forget_without_staged_events_does_not_run_hook() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = CustomersWithHook::new(pool);

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Quiet Quinn")
        .email("quiet@example.com")
        .build()
        .unwrap();
    let customer = customers.create(new_customer).await?;
    assert_eq!(customers.calls().len(), 1);

    // No staged events: nothing is persisted, so there is nothing to publish
    // — mirroring `update`'s no-op semantics.
    customers.forget(customer).await?;
    assert_eq!(customers.calls().len(), 1, "hook must not fire");

    Ok(())
}

#[tokio::test]
async fn hook_sees_forgotten_payloads_for_staged_pii_events() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = CustomersWithHook::new(pool);

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Leaky Lou")
        .email("leaky@example.com")
        .build()
        .unwrap();
    let mut customer = customers.create(new_customer).await?;

    // A staged-but-unpersisted PII event rides along with the erasure. The
    // hook must receive it in its forgotten form — the raw value must never
    // reach a publisher during an erasure.
    let _ = customer.update_name("Unpersisted Raw Name");
    customer.record_erasure();
    customers.forget(customer).await?;

    let calls = customers.calls();
    assert_eq!(calls.len(), 2);
    let forget_call = &calls[1];
    assert_eq!(forget_call.event_types, vec!["name_updated", "forgot"]);
    assert!(
        !forget_call.saw_raw_name,
        "hook must never observe raw PII during an erasure"
    );
    assert_eq!(forget_call.entity_name, "[forgotten]");

    Ok(())
}

#[tokio::test]
async fn hook_error_rolls_back_the_entire_erasure() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = CustomersWithHook::new(pool.clone());

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Rollback Rae")
        .email("rollback@example.com")
        .build()
        .unwrap();
    let mut customer = customers.create(new_customer).await?;

    customers.fail_on_forgot.store(true, Ordering::SeqCst);
    customer.record_erasure();
    let err = match customers.forget(customer).await {
        Err(e) => e,
        Ok(_) => panic!("hook failure must fail the forget"),
    };
    match err {
        CustomerForgetError::PostPersistHookError(inner) => {
            assert!(inner.to_string().contains("rejected forgot event"));
        }
        e => panic!("expected PostPersistHookError, got: {e}"),
    }

    // The whole erasure rolled back: payloads still present, entity still
    // live, no forgot event persisted.
    let payloads = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!" FROM customers_forgettable_payloads
           WHERE entity_id = $1"#,
        id as CustomerId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(payloads.count, 1);

    let reloaded = customers.find_by_id(id).await?;
    assert_eq!(reloaded.name, "Rollback Rae");
    let forgot_events = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!" FROM customer_events
           WHERE id = $1 AND event_type = 'forgot'"#,
        id as CustomerId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(forgot_events.count, 0);

    // Retry after the publisher recovers: the erasure is retryable.
    customers.fail_on_forgot.store(false, Ordering::SeqCst);
    let mut customer = customers.find_by_id(id).await?;
    customer.record_erasure();
    let customer = customers.forget(customer).await?;
    assert_eq!(customer.name, "[forgotten]");
    customers.verify_forgotten(id).await?;

    Ok(())
}

#[tokio::test]
async fn stale_writer_pii_is_fenced_and_leaves_no_trace_after_forget() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = CustomersWithHook::new(pool.clone());

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Fenced Fran")
        .email("fenced@example.com")
        .build()
        .unwrap();
    customers.create(new_customer).await?;

    let mut stale = customers.find_by_id(id).await?;
    let mut fresh = customers.find_by_id(id).await?;

    fresh.record_erasure();
    customers.forget(fresh).await?;

    // A stale instance trying to write PII after the concurrent forget is
    // rejected by the sequence fence...
    let _ = stale.update_name("Resurrected PII");
    let err = customers
        .update(&mut stale)
        .await
        .expect_err("stale PII write after forget must fail");
    assert!(err.was_concurrent_modification());

    // ...the rejected write publishes nothing...
    let calls = customers.calls();
    assert_eq!(
        calls.len(),
        2,
        "create + forget only — no publish for the rejected write"
    );

    // ...and leaves no PII behind at the storage level.
    customers.verify_forgotten(id).await?;

    Ok(())
}
