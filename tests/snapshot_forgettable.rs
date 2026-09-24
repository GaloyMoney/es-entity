mod entities;
mod helpers;

use entities::client::*;
use es_entity::*;
use sqlx::PgPool;

/// Snapshot + forgettable together: the snapshot's own `Forgettable` fields
/// live in the same `<tbl>_forgettable_payloads` table, at the reserved
/// `sequence = 0` row.
#[derive(EsRepo, Debug)]
#[es_repo(entity = "Client", snapshot, forgettable)]
pub struct ClientRepo {
    pool: PgPool,
}

impl ClientRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn new_client(email: &str) -> NewClient {
    NewClient::builder()
        .id(ClientId::new())
        .email(email)
        .build()
        .unwrap()
}

async fn payload_row(pool: &PgPool, id: ClientId) -> anyhow::Result<Option<String>> {
    let row = sqlx::query!(
        r#"SELECT payload->>'email' AS email FROM clients_forgettable_payloads
           WHERE entity_id = $1 AND sequence = 0"#,
        id as ClientId
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.and_then(|r| r.email))
}

async fn payload_row_count(pool: &PgPool, id: ClientId) -> anyhow::Result<i64> {
    Ok(sqlx::query!(
        r#"SELECT COUNT(*) AS "count!" FROM clients_forgettable_payloads WHERE entity_id = $1"#,
        id as ClientId
    )
    .fetch_one(pool)
    .await?
    .count)
}

async fn snapshot_email(pool: &PgPool, id: ClientId) -> anyhow::Result<Option<Option<String>>> {
    let row = sqlx::query!(
        r#"SELECT snapshot->>'email' AS email FROM client_snapshots WHERE id = $1"#,
        id as ClientId
    )
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| r.email))
}

/// A snapshot write stores the forgettable field at the reserved
/// `sequence = 0` row, and a reload folds it back in.
#[tokio::test]
async fn snapshot_writes_forgettable_payload_at_sequence_zero() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = ClientRepo::new(pool.clone());

    let mut client = repo.create(new_client("alice@example.com")).await?;
    // `Client::capture()` fires once `tail_len() >= 2` — Initialized plus
    // one change is already there.
    let _ = client.change_email("bob@example.com");
    repo.update(&mut client).await?;
    assert!(client.has_snapshot());

    assert_eq!(
        payload_row(&pool, client.id).await?,
        Some("bob@example.com".to_string())
    );

    let reloaded = repo.find_by_id(client.id).await?;
    assert!(reloaded.has_snapshot());
    assert_eq!(reloaded.email(), Some("bob@example.com".to_string()));

    Ok(())
}

/// `forget` on a snapshotted, forgettable entity: the payload row (including
/// the reserved sequence-0 one) and the old snapshot row are both gone; the
/// repo immediately re-snapshots with a forgotten email baked in, and
/// `verify_forgotten` passes.
#[tokio::test]
async fn forget_forgets_the_snapshot_and_resnapshots() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = ClientRepo::new(pool.clone());

    let mut client = repo.create(new_client("dana@example.com")).await?;
    let _ = client.change_email("erin@example.com");
    repo.update(&mut client).await?;
    assert!(client.has_snapshot());
    assert!(payload_row(&pool, client.id).await?.is_some());

    let loaded = repo.find_by_id(client.id).await?;
    let forgotten = repo.forget(loaded).await?;
    assert_eq!(forgotten.email(), None);
    assert!(
        forgotten.has_snapshot(),
        "forget_in_op re-snapshots immediately"
    );

    repo.verify_forgotten(client.id).await?;

    // No sequence-0 (or any) payload row survives.
    assert_eq!(payload_row_count(&pool, client.id).await?, 0);

    // A snapshot row exists again, with a null (forgotten) email.
    assert_eq!(snapshot_email(&pool, client.id).await?, Some(None));

    let reloaded = repo.find_by_id(client.id).await?;
    assert_eq!(reloaded.email(), None);
    assert!(reloaded.has_snapshot());

    Ok(())
}

/// Same as above, but the snapshot row present before `forget` has a stale
/// (mismatched) fingerprint. `forget_in_op` rebuilds from full history
/// regardless, so the outcome is identical.
#[tokio::test]
async fn forget_with_a_stale_fingerprint_snapshot_still_resnapshots_cleanly() -> anyhow::Result<()>
{
    let pool = helpers::init_pool().await?;
    let repo = ClientRepo::new(pool.clone());

    let mut client = repo.create(new_client("frank@example.com")).await?;
    let _ = client.change_email("georgia@example.com");
    repo.update(&mut client).await?;
    assert!(client.has_snapshot());

    sqlx::query!(
        "UPDATE client_snapshots SET fingerprint = fingerprint + 1 WHERE id = $1",
        client.id as ClientId
    )
    .execute(&pool)
    .await?;

    let loaded = repo.find_by_id(client.id).await?;
    assert!(
        !loaded.has_snapshot(),
        "the mismatched fingerprint must be ignored on load"
    );
    let forgotten = repo.forget(loaded).await?;
    assert_eq!(forgotten.email(), None);

    repo.verify_forgotten(client.id).await?;
    assert_eq!(payload_row_count(&pool, client.id).await?, 0);
    assert_eq!(snapshot_email(&pool, client.id).await?, Some(None));

    let reloaded = repo.find_by_id(client.id).await?;
    assert_eq!(reloaded.email(), None);
    assert!(reloaded.has_snapshot());

    Ok(())
}

/// An ordinary write (not `forget`) whose staged event sets the forgettable
/// field to an already-forgotten value: once that crosses the snapshot
/// threshold, `capture()` is `Some` but its own
/// `extract_forgettable_payloads()` is `None` (nothing left to store), which
/// must delete the stale `sequence = 0` row from the *previous* snapshot
/// rather than leaving it behind.
#[tokio::test]
async fn email_changed_to_a_forgotten_value_deletes_the_stale_payload_row() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = ClientRepo::new(pool.clone());

    let mut client = repo.create(new_client("henry@example.com")).await?;
    let _ = client.change_email("iris@example.com");
    repo.update(&mut client).await?;
    assert!(client.has_snapshot());
    assert_eq!(
        payload_row(&pool, client.id).await?,
        Some("iris@example.com".to_string())
    );

    // One more (real) change: tail grows to 1, below the threshold again —
    // no new snapshot yet.
    let _ = client.change_email("jack@example.com");
    repo.update(&mut client).await?;
    assert_eq!(client.tail_len(), 1);

    // Stage a forgotten value directly (not through `forget()`): tail_len()
    // reaches 2 again, so this write snapshots — with a `None` payload.
    client.events_mut().push(ClientEvent::EmailChanged {
        email: Forgettable::forgotten(),
    });
    repo.update(&mut client).await?;
    assert!(client.has_snapshot());

    // The stale sequence-0 row (from the earlier real snapshot) is gone —
    // the per-event payload rows at sequence 1 ("henry", `Initialized`), 2
    // ("iris"), and 3 ("jack") are untouched: they are the events' own
    // historical payloads, not the snapshot's, and this is an ordinary
    // update, not a `forget`.
    assert_eq!(
        payload_row(&pool, client.id).await?,
        None,
        "the DELETE ... sequence = 0 path must run when a fresh snapshot has no payload"
    );
    assert_eq!(payload_row_count(&pool, client.id).await?, 3);
    assert_eq!(snapshot_email(&pool, client.id).await?, Some(None));

    let reloaded = repo.find_by_id(client.id).await?;
    assert_eq!(reloaded.email(), None);

    Ok(())
}
