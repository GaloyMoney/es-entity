mod entities;
mod helpers;

use entities::meter::*;
use es_entity::*;
use sqlx::PgPool;

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Meter",
    snapshot,
    snapshot_tbl = "meter_snapshots",
    columns(label(ty = "String"))
)]
pub struct MeterRepo {
    pool: PgPool,
}

impl MeterRepo {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "PlainMeter",
    id = "MeterId",
    event = "MeterEvent",
    tbl = "meters",
    events_tbl = "meter_events",
    columns(label(ty = "String"))
)]
pub struct PlainMeterRepo {
    pool: PgPool,
}

impl PlainMeterRepo {
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

/// Round trip: create, 10 `record`s via `update` → reload: has a snapshot,
/// a short tail, and folds to the same state as a full replay of the same
/// rows via the non-snapshot twin; `len_persisted()` equals the events row
/// count.
#[tokio::test]
async fn round_trip() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool.clone());
    let plain = PlainMeterRepo::new(pool.clone());

    let mut meter = repo.create(new_meter("m1")).await?;
    for v in 1..=10i64 {
        let _ = meter.record(v);
        repo.update(&mut meter).await?;
    }

    let reloaded = repo.find_by_id(meter.id).await?;
    assert!(reloaded.has_snapshot(), "10 events should have snapshotted");
    assert!(
        reloaded.tail_len() < 4,
        "tail should be short after a snapshot"
    );

    let full = plain.find_by_id(meter.id).await?;
    assert_eq!(reloaded.total(), full.total());
    assert_eq!(reloaded.count(), full.count());
    assert_eq!(reloaded.last_value(), full.last_value());
    assert_eq!(reloaded.count(), 10);
    assert_eq!(reloaded.total(), 55);
    assert_eq!(reloaded.last_value(), Some(10));

    let row_count = sqlx::query!(
        "SELECT COUNT(*) AS \"count!\" FROM meter_events WHERE id = $1",
        meter.id as MeterId
    )
    .fetch_one(&pool)
    .await?
    .count;
    assert_eq!(reloaded.len_persisted() as i64, row_count);

    Ok(())
}

/// Snapshot + tail + guard across the boundary: `record(v)` already applied
/// when `v` is only in the snapshot, when only in the tail, and re-applied
/// after a different reading (`resets_on`).
#[tokio::test]
async fn guard_across_snapshot_boundary() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool);

    let mut meter = repo.create(new_meter("m2")).await?;
    // First 4 readings will be folded into the snapshot.
    for v in [1, 2, 3, 4] {
        let _ = meter.record(v);
    }
    repo.update(&mut meter).await?;
    assert!(meter.has_snapshot());

    // Value only in the snapshot (the last folded reading, 4).
    assert!(meter.record(4).was_already_applied());

    // New reading goes to the tail.
    let _ = meter.record(5);
    repo.update(&mut meter).await?;
    assert!(!meter.has_snapshot() || meter.tail_len() > 0);

    // Value only in the tail.
    assert!(meter.record(5).was_already_applied());

    // resets_on: a different reading resets the guard, so re-recording 5 now executes.
    let _ = meter.record(6);
    assert!(meter.record(5).did_execute());

    Ok(())
}

/// Forward guard: `reset()` idempotency reads the snapshot first, then the
/// tail.
#[tokio::test]
async fn forward_guard_reads_snapshot_first() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool);

    let mut meter = repo.create(new_meter("m3")).await?;
    for v in [1, 2, 3, 4] {
        let _ = meter.record(v);
    }
    assert!(meter.reset().did_execute());
    repo.update(&mut meter).await?;
    assert!(
        meter.has_snapshot(),
        "5 staged events should snapshot on this update"
    );

    let mut reloaded = repo.find_by_id(meter.id).await?;
    assert!(reloaded.reset().was_already_applied());

    Ok(())
}

/// Fingerprint mismatch ⇒ full replay, self-heals: after flipping the
/// stored fingerprint, a reload ignores the snapshot (falls back to a full
/// replay with correct state); the next update rewrites the row with the
/// right fingerprint.
#[tokio::test]
async fn fingerprint_mismatch_self_heals() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool.clone());

    let mut meter = repo.create(new_meter("m4")).await?;
    for v in 1..=5i64 {
        let _ = meter.record(v);
        repo.update(&mut meter).await?;
    }
    assert!(meter.has_snapshot());

    sqlx::query!(
        "UPDATE meter_snapshots SET fingerprint = fingerprint + 1 WHERE id = $1",
        meter.id as MeterId
    )
    .execute(&pool)
    .await?;

    let reloaded = repo.find_by_id(meter.id).await?;
    assert!(
        !reloaded.has_snapshot(),
        "a fingerprint mismatch must be ignored, not fatal"
    );
    assert_eq!(reloaded.count(), 5);
    assert_eq!(reloaded.total(), 15);

    let mut reloaded = reloaded;
    let _ = reloaded.record(6);
    repo.update(&mut reloaded).await?;

    let row = sqlx::query!(
        "SELECT fingerprint FROM meter_snapshots WHERE id = $1",
        meter.id as MeterId
    )
    .fetch_optional(&pool)
    .await?;
    if let Some(row) = row {
        assert_eq!(
            row.fingerprint,
            <MeterSnapshot as EsSnapshot>::FINGERPRINT,
            "the next write should rewrite the row with the current fingerprint"
        );
    }

    Ok(())
}

/// Corrupt blob with matching fingerprint ⇒ `SnapshotDecode`.
#[tokio::test]
async fn corrupt_snapshot_blob_is_a_hard_error() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool.clone());

    let mut meter = repo.create(new_meter("m5")).await?;
    for v in 1..=5i64 {
        let _ = meter.record(v);
        repo.update(&mut meter).await?;
    }
    assert!(meter.has_snapshot());

    sqlx::query!(
        "UPDATE meter_snapshots SET snapshot = '{}'::jsonb WHERE id = $1",
        meter.id as MeterId
    )
    .execute(&pool)
    .await?;

    let err = match repo.find_by_id(meter.id).await {
        Ok(_) => panic!("expected a SnapshotDecode error, got Ok"),
        Err(e) => e,
    };
    assert!(
        matches!(
            err,
            MeterFindError::HydrationError(EntityHydrationError::SnapshotDecode { .. })
        ),
        "unexpected error: {err}"
    );

    // Leave the row readable again — an unscoped scan in another test
    // (`list_by_id` in `batch_load_mixes_snapshot_states`) would otherwise
    // trip over this permanently corrupted row for the rest of the suite's
    // lifetime against this database.
    sqlx::query!(
        "DELETE FROM meter_snapshots WHERE id = $1",
        meter.id as MeterId
    )
    .execute(&pool)
    .await?;

    Ok(())
}

/// Compaction: after an update that snapshotted, the tail is empty and
/// `len_persisted()` unchanged; a further update on the same in-memory
/// entity persists with correct sequences (no `UNIQUE` violation).
#[tokio::test]
async fn compaction_after_snapshot_write() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool.clone());

    let mut meter = repo.create(new_meter("m7")).await?;
    for v in [1, 2, 3, 4] {
        let _ = meter.record(v);
    }
    let len_before = meter.len_persisted() + 4;
    repo.update(&mut meter).await?;

    assert!(meter.has_snapshot());
    assert_eq!(meter.tail_len(), 0, "the tail must be compacted away");
    assert_eq!(meter.len_persisted(), len_before);

    // A further update on the SAME in-memory (already-compacted) entity
    // must not violate UNIQUE(id, sequence).
    let _ = meter.record(5);
    repo.update(&mut meter).await?;
    assert_eq!(meter.count(), 5);

    let row_count = sqlx::query!(
        "SELECT COUNT(*) AS \"count!\" FROM meter_events WHERE id = $1",
        meter.id as MeterId
    )
    .fetch_one(&pool)
    .await?
    .count;
    assert_eq!(meter.len_persisted() as i64, row_count);

    Ok(())
}

/// `capture()` returning `None` keeps the old row and grows the tail (row
/// `sequence` unchanged).
#[tokio::test]
async fn snapshot_returning_none_keeps_old_row() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool.clone());

    let mut meter = repo.create(new_meter("m8")).await?;
    for v in [1, 2, 3] {
        let _ = meter.record(v);
        repo.update(&mut meter).await?;
    }
    assert!(meter.has_snapshot());
    let seq_before = sqlx::query!(
        "SELECT sequence FROM meter_snapshots WHERE id = $1",
        meter.id as MeterId
    )
    .fetch_one(&pool)
    .await?
    .sequence;

    // One more reading: tail_len() goes from 0 to 1, capture() returns None.
    let _ = meter.record(4);
    repo.update(&mut meter).await?;
    assert_eq!(meter.tail_len(), 1);

    let seq_after = sqlx::query!(
        "SELECT sequence FROM meter_snapshots WHERE id = $1",
        meter.id as MeterId
    )
    .fetch_one(&pool)
    .await?
    .sequence;
    assert_eq!(
        seq_before, seq_after,
        "the old snapshot row must be untouched"
    );

    Ok(())
}

/// Batch: `find_all` and `list_by_id` over a page mixing snapshotted,
/// never-snapshotted, and stale-fingerprint meters — every entity correct.
#[tokio::test]
async fn batch_load_mixes_snapshot_states() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool.clone());

    // Snapshotted.
    let mut snapshotted = repo.create(new_meter("snapshotted")).await?;
    for v in 1..=5i64 {
        let _ = snapshotted.record(v);
        repo.update(&mut snapshotted).await?;
    }
    assert!(snapshotted.has_snapshot());

    // Never snapshotted (tail stays short).
    let mut fresh = repo.create(new_meter("fresh")).await?;
    let _ = fresh.record(1);
    repo.update(&mut fresh).await?;
    assert!(!fresh.has_snapshot());

    // Stale fingerprint.
    let mut stale = repo.create(new_meter("stale")).await?;
    for v in 1..=5i64 {
        let _ = stale.record(v);
        repo.update(&mut stale).await?;
    }
    assert!(stale.has_snapshot());
    sqlx::query!(
        "UPDATE meter_snapshots SET fingerprint = fingerprint + 1 WHERE id = $1",
        stale.id as MeterId
    )
    .execute(&pool)
    .await?;

    let ids = [snapshotted.id, fresh.id, stale.id];
    let all = repo.find_all::<Meter>(&ids).await?;
    assert_eq!(all.len(), 3);
    assert_eq!(all[&snapshotted.id].count(), 5);
    assert_eq!(all[&fresh.id].count(), 1);
    assert_eq!(all[&stale.id].count(), 5);
    assert!(
        !all[&stale.id].has_snapshot(),
        "stale fingerprint must be ignored"
    );

    // Page through `list_by_id` (unscoped — it walks every meter this
    // database has ever seen, across every test in this suite) until all
    // three planted ids have turned up, rather than assuming they land on
    // the first page.
    let mut found: std::collections::HashSet<MeterId> = std::collections::HashSet::new();
    let mut after = None;
    loop {
        let page = repo
            .list_by_id(
                es_entity::PaginatedQueryArgs { first: 100, after },
                es_entity::ListDirection::Ascending,
            )
            .await?;
        found.extend(page.entities().iter().map(|m| m.id));
        let done = ids.iter().all(|id| found.contains(id)) || !page.has_next_page();
        after = page.into_end_cursor();
        if done {
            break;
        }
    }
    for id in ids {
        assert!(found.contains(&id));
    }

    Ok(())
}

/// A clean entity whose loaded state has no matching snapshot — because a
/// fingerprint change made a fresh row stop matching — gets refreshed on
/// its next `update`, even though that call stages no new events.
#[tokio::test]
async fn stale_clean_entity_refreshes_on_next_update() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = MeterRepo::new(pool.clone());

    let mut meter = repo.create(new_meter("m11")).await?;
    for v in 1..=4i64 {
        let _ = meter.record(v);
        repo.update(&mut meter).await?;
    }
    assert!(meter.has_snapshot());

    sqlx::query!(
        "UPDATE meter_snapshots SET fingerprint = fingerprint + 1 WHERE id = $1",
        meter.id as MeterId
    )
    .execute(&pool)
    .await?;

    let mut reloaded = repo.find_by_id(meter.id).await?;
    assert!(!reloaded.has_snapshot(), "the stale row must be ignored");

    // No events staged — `update` is a pure refresh.
    assert!(!reloaded.events().any_new());
    let n = repo.update(&mut reloaded).await?;
    assert_eq!(n, 0, "a pure refresh persists no events");

    let row = sqlx::query!(
        "SELECT fingerprint FROM meter_snapshots WHERE id = $1",
        meter.id as MeterId
    )
    .fetch_one(&pool)
    .await?;
    assert_eq!(
        row.fingerprint,
        <MeterSnapshot as EsSnapshot>::FINGERPRINT,
        "the refresh must rewrite the row with the current fingerprint"
    );

    let reloaded_again = repo.find_by_id(meter.id).await?;
    assert!(
        reloaded_again.has_snapshot(),
        "the refreshed row must now match"
    );

    Ok(())
}
