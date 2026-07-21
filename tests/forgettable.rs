mod entities;
mod helpers;

use entities::customer::*;
use es_entity::*;
use sqlx::PgPool;

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Customer", forgettable, columns(email(ty = "String")))]
pub struct Customers {
    pool: PgPool,
}

impl Customers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn create_and_load_with_forgettable_fields() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let new_customer = NewCustomer::builder()
        .id(CustomerId::new())
        .name("Alice Smith")
        .email("alice@example.com")
        .build()
        .unwrap();

    let customer = customers.create(new_customer).await?;
    assert_eq!(customer.name, "Alice Smith");
    assert_eq!(customer.email, "alice@example.com");

    // Load the customer and verify data is intact
    let loaded = customers.find_by_id(customer.id).await?;
    assert_eq!(loaded.name, "Alice Smith");
    assert_eq!(loaded.email, "alice@example.com");

    Ok(())
}

#[tokio::test]
async fn forget_removes_forgettable_data() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Bob Jones")
        .email("bob@example.com")
        .build()
        .unwrap();

    let mut customer = customers.create(new_customer).await?;
    assert_eq!(customer.name, "Bob Jones");

    // Update the name (adds another event with a forgettable field)
    let _ = customer.update_name("Robert Jones");
    customers.update(&mut customer).await?;

    // Verify before forget
    let loaded = customers.find_by_id(id).await?;
    assert_eq!(loaded.name, "Robert Jones");
    assert_eq!(loaded.email, "bob@example.com");

    // Forget the customer's personal data - consumes and returns the rebuilt,
    // forgotten entity.
    let loaded = customers.find_by_id(id).await?;
    let loaded = customers.forget(loaded).await?;

    assert_eq!(loaded.name, "[forgotten]");
    // Non-forgettable field should remain intact
    assert_eq!(loaded.email, "bob@example.com");

    Ok(())
}

#[tokio::test]
async fn forget_preserves_non_forgettable_events() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Charlie")
        .email("charlie@example.com")
        .build()
        .unwrap();

    let mut customer = customers.create(new_customer).await?;

    // Update email (non-forgettable field)
    let _ = customer.update_email("charlie_new@example.com");
    customers.update(&mut customer).await?;

    // Forget and verify - forget consumes and returns the rebuilt, forgotten
    // entity.
    let loaded = customers.find_by_id(id).await?;
    let loaded = customers.forget(loaded).await?;

    assert_eq!(loaded.name, "[forgotten]");
    assert_eq!(loaded.email, "charlie_new@example.com");

    Ok(())
}

#[tokio::test]
async fn staged_erasure_event_is_persisted_by_forget() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool.clone());

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Marker Test")
        .email("marker@example.com")
        .build()
        .unwrap();

    let mut customer = customers.create(new_customer).await?;

    // Convention: stage the domain erasure event before forgetting. forget()
    // persists it in the erasure transaction — durable Art. 17 evidence
    // in-stream, visible to outbox consumers via the normal pipeline, and a
    // consumed sequence number that fences stale writers.
    customer.record_erasure();
    let mut customer = customers.forget(customer).await?;

    let row = sqlx::query!(
        r#"SELECT event_type, event FROM customer_events
           WHERE id = $1 ORDER BY sequence DESC LIMIT 1"#,
        id as CustomerId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(row.event_type, "forgot");
    assert_eq!(row.event, serde_json::json!({ "type": "forgot" }));

    // The rebuilt entity carries the erasure event in its stream.
    assert!(
        customer
            .events()
            .iter_all()
            .any(|e| matches!(e, CustomerEvent::Forgot { .. }))
    );

    // Repeat forgets are legitimate: multiple erasure events = true history.
    customer.record_erasure();
    customers.forget(customer).await?;
    let count = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!" FROM customer_events
           WHERE id = $1 AND event_type = 'forgot'"#,
        id as CustomerId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(count.count, 2);

    Ok(())
}

#[tokio::test]
async fn staged_erasure_event_fences_stale_writers() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Stale Writer")
        .email("stale@example.com")
        .build()
        .unwrap();
    customers.create(new_customer).await?;

    // Two copies of the same entity: one gets forgotten, one goes stale.
    let mut stale = customers.find_by_id(id).await?;
    let mut fresh = customers.find_by_id(id).await?;

    // The staged erasure event consumes the next sequence number when
    // forget() persists it — that consumption is the concurrency fence.
    fresh.record_erasure();
    customers.forget(fresh).await?;

    let _ = stale.update_name("Resurrected Name");
    let err = customers
        .update(&mut stale)
        .await
        .expect_err("stale update after fenced forget must fail");
    assert!(
        err.was_concurrent_modification(),
        "expected ConcurrentModification, got: {err}"
    );

    // And the forgotten value stays forgotten.
    let reloaded = customers.find_by_id(id).await?;
    assert_eq!(reloaded.name, "[forgotten]");

    Ok(())
}

#[tokio::test]
async fn forget_without_staged_event_leaves_stale_writers_unfenced() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Unfenced")
        .email("unfenced@example.com")
        .build()
        .unwrap();
    customers.create(new_customer).await?;

    let mut stale = customers.find_by_id(id).await?;
    let fresh = customers.find_by_id(id).await?;

    // No erasure event staged: forget() consumes no sequence number.
    customers.forget(fresh).await?;

    // This pins the documented tradeoff of convention-based erasure: without
    // a staged erasure event there is nothing to fence a stale writer, so
    // its update() succeeds and re-persists the data that was just
    // forgotten. Stage a domain erasure event before forget() (see the
    // book chapter) to close this race.
    let _ = stale.update_name("Resurrected Name");
    customers
        .update(&mut stale)
        .await
        .expect("unfenced stale update succeeds — the accepted tradeoff");

    let reloaded = customers.find_by_id(id).await?;
    assert_eq!(reloaded.name, "Resurrected Name");

    Ok(())
}

#[tokio::test]
async fn forget_persists_staged_events_without_laundering() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool.clone());

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Pending Pat")
        .email("pending@example.com")
        .build()
        .unwrap();
    let mut customer = customers.create(new_customer).await?;

    // Mutate without persisting: the staged event carries a raw forgettable
    // value in memory. forget() persists it BEFORE deleting payload rows,
    // so the payload row that persistence inserts is hard-deleted in the
    // same transaction — the raw value never survives the erasure.
    let _ = customer.update_name("Unpersisted Name");
    let customer = customers.forget(customer).await?;
    assert_eq!(customer.name, "[forgotten]");

    // The staged event was persisted with null in the durable JSON...
    let row = sqlx::query!(
        r#"SELECT event->>'name' IS NULL AS "name_is_null!" FROM customer_events
           WHERE id = $1 AND event_type = 'name_updated'"#,
        id as CustomerId
    )
    .fetch_one(&pool)
    .await?;
    assert!(row.name_is_null);

    // ...and no payload row survived the erasure transaction.
    let payloads = sqlx::query!(
        r#"SELECT COUNT(*) AS "count!" FROM customers_forgettable_payloads
           WHERE entity_id = $1"#,
        id as CustomerId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(payloads.count, 0);

    Ok(())
}

#[tokio::test]
async fn events_table_stores_null_for_live_forgettable_fields() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool.clone());

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Nina Null")
        .email("nina@example.com")
        .build()
        .unwrap();

    let customer = customers.create(new_customer).await?;
    // The live entity has the value...
    assert_eq!(customer.name, "Nina Null");

    // ...but the durable event JSON must never contain the raw PII, even
    // while the entity is live: the `name` key holds JSON null.
    let row = sqlx::query!(
        r#"SELECT
             event->>'name' IS NULL AS "name_is_null!",
             event ? 'name' AS "name_key_present!",
             event->>'email' AS email
           FROM customer_events
           WHERE id = $1 AND sequence = 1"#,
        id as CustomerId
    )
    .fetch_one(&pool)
    .await?;
    assert!(row.name_is_null, "raw PII leaked into events table");
    assert!(row.name_key_present, "name key should be present (as null)");
    assert_eq!(row.email.as_deref(), Some("nina@example.com"));

    // The real value lives in the payloads table instead.
    let payload = sqlx::query!(
        r#"SELECT payload->>'name' AS "name!" FROM customers_forgettable_payloads
           WHERE entity_id = $1 AND sequence = 1"#,
        id as CustomerId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(payload.name, "Nina Null");

    Ok(())
}

#[tokio::test]
async fn create_all_stores_payloads_and_nulls_events() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool.clone());

    let id1 = CustomerId::new();
    let id2 = CustomerId::new();
    let new_customers = vec![
        NewCustomer::builder()
            .id(id1)
            .name("Batch One")
            .email("batch1@example.com")
            .build()
            .unwrap(),
        NewCustomer::builder()
            .id(id2)
            .name("Batch Two")
            .email("batch2@example.com")
            .build()
            .unwrap(),
    ];

    let created = customers.create_all(new_customers).await?;
    assert_eq!(created.len(), 2);

    // Batch persistence must extract payloads exactly like the single path:
    // events hold null, payload rows hold the values.
    for (id, expected_name) in [(id1, "Batch One"), (id2, "Batch Two")] {
        let row = sqlx::query!(
            r#"SELECT event->>'name' IS NULL AS "name_is_null!" FROM customer_events
               WHERE id = $1 AND sequence = 1"#,
            id as CustomerId
        )
        .fetch_one(&pool)
        .await?;
        assert!(row.name_is_null, "raw PII leaked into events table (batch)");

        let payload = sqlx::query!(
            r#"SELECT payload->>'name' AS "name!" FROM customers_forgettable_payloads
               WHERE entity_id = $1 AND sequence = 1"#,
            id as CustomerId
        )
        .fetch_one(&pool)
        .await?;
        assert_eq!(payload.name, expected_name);

        // And loading hydrates the value back.
        let loaded = customers.find_by_id(id).await?;
        assert_eq!(loaded.name, expected_name);
    }

    Ok(())
}

#[tokio::test]
async fn find_all_works_with_forgettable() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let id1 = CustomerId::new();
    let id2 = CustomerId::new();

    let c1 = NewCustomer::builder()
        .id(id1)
        .name("Dave")
        .email("dave@example.com")
        .build()
        .unwrap();
    let c2 = NewCustomer::builder()
        .id(id2)
        .name("Eve")
        .email("eve@example.com")
        .build()
        .unwrap();

    customers.create(c1).await?;
    customers.create(c2).await?;

    let all = customers.find_all::<Customer>(&[id1, id2]).await?;
    assert_eq!(all.len(), 2);
    assert_eq!(all[&id1].name, "Dave");
    assert_eq!(all[&id2].name, "Eve");

    // Forget one customer - forget consumes and returns the rebuilt, forgotten
    // entity.
    let c1 = customers.find_by_id(id1).await?;
    let c1 = customers.forget(c1).await?;
    assert_eq!(c1.name, "[forgotten]");

    let all = customers.find_all::<Customer>(&[id1, id2]).await?;
    assert_eq!(all[&id1].name, "[forgotten]");
    assert_eq!(all[&id2].name, "Eve");

    Ok(())
}
