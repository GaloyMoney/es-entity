mod entities;
mod helpers;

use entities::meter::*;
use es_entity::*;
use sqlx::PgPool;

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Meter", snapshot, columns(label(ty = "String")))]
pub struct MeterRepo {
    pool: PgPool,
}

impl MeterRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn new_meter(label: &str) -> NewMeter {
    NewMeter::builder()
        .id(MeterId::new())
        .site_id(SiteId::new())
        .label(label)
        .build()
        .unwrap()
}

/// Concurrency: two independently loaded copies of the same meter,
/// each staging one more reading, `update()` at the same time on separate
/// connections (a real multi-threaded tokio runtime, two `spawn`ed tasks —
/// no `sleep`, no artificial sequencing). Exactly one wins; the loser gets
/// `ConcurrentModification`; the winner's write is the one that crosses the
/// snapshot threshold, and `meter_snapshots.sequence` ends up exactly
/// `MAX(meter_events.sequence)`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_updates_one_wins_and_the_snapshot_matches_the_winner() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool.clone());

    let mut seed = repo.create(new_meter("race")).await?;
    // Two readings already in: tail is 3 (Initialized + 2 readings), one
    // short of the snapshot threshold (4). Whichever concurrent writer wins
    // adds the 4th and crosses it; the loser's whole statement (events +
    // snapshot CTE) rolls back atomically on the `UNIQUE(id, sequence)`
    // violation, so no partial/incorrect snapshot can ever land.
    for v in [1, 2] {
        let _ = seed.record(v);
        repo.update(&mut seed).await?;
    }
    assert!(!seed.has_snapshot(), "tail stays below the threshold here");
    let id = seed.id;

    let mut a = repo.find_by_id(id).await?;
    let mut b = repo.find_by_id(id).await?;
    let _ = a.record(100);
    let _ = b.record(200);

    let repo_a = MeterRepo::new(pool.clone());
    let repo_b = MeterRepo::new(pool.clone());

    let task_a = tokio::spawn(async move {
        let res = repo_a.update(&mut a).await;
        (res, a)
    });
    let task_b = tokio::spawn(async move {
        let res = repo_b.update(&mut b).await;
        (res, b)
    });

    let (res_a, a) = task_a.await?;
    let (res_b, b) = task_b.await?;

    let results = [res_a.is_ok(), res_b.is_ok()];
    assert_eq!(
        results.iter().filter(|ok| **ok).count(),
        1,
        "exactly one of the two concurrent writers must succeed: {results:?}"
    );

    for res in [&res_a, &res_b] {
        if let Err(e) = res {
            assert!(
                e.was_concurrent_modification(),
                "the loser must fail with ConcurrentModification, got: {e}"
            );
        }
    }

    let winner_value = if res_a.is_ok() {
        a.last_value()
    } else {
        b.last_value()
    };
    assert!(
        winner_value == Some(100) || winner_value == Some(200),
        "the winner's own staged reading must have landed: {winner_value:?}"
    );

    let max_event_seq = sqlx::query!(
        r#"SELECT MAX(sequence) AS "max!" FROM meter_events WHERE id = $1"#,
        id as MeterId
    )
    .fetch_one(&pool)
    .await?
    .max;
    let snapshot_seq = sqlx::query!(
        "SELECT sequence FROM meter_snapshots WHERE id = $1",
        id as MeterId
    )
    .fetch_optional(&pool)
    .await?
    .map(|r| r.sequence);

    assert_eq!(
        snapshot_seq,
        Some(max_event_seq),
        "the snapshot the winner wrote must sit exactly at the current head"
    );

    // Reloading must see exactly the winner's state — the loser's write
    // never touched the database at all (rolled back with the statement).
    let reloaded = repo.find_by_id(id).await?;
    assert_eq!(reloaded.last_value(), winner_value);
    assert!(reloaded.has_snapshot());

    Ok(())
}
