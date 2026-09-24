#![cfg(feature = "instrument")]

mod entities;
mod helpers;

use std::sync::{Arc, Mutex};

use entities::{meter::*, site::*};
use es_entity::*;
use helpers::init_pool;
use sqlx::PgPool;
use tracing_subscriber::layer::SubscriberExt;

/// Both parent (`Site`) and child (`Meter`) snapshot.
#[derive(EsRepo, Debug)]
#[es_repo(entity = "Site", snapshot)]
pub struct BothSites {
    pool: PgPool,

    #[es_repo(nested)]
    meters: BothMeters,
}

impl BothSites {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            meters: BothMeters::new(pool),
        }
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Meter",
    snapshot,
    columns(
        site_id(ty = "SiteId", update(persist = false), parent),
        label(ty = "String")
    )
)]
pub struct BothMeters {
    pool: PgPool,
}

impl BothMeters {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(Clone, Default)]
struct QueryEventCount(Arc<Mutex<usize>>);

impl QueryEventCount {
    fn get(&self) -> usize {
        *self.0.lock().unwrap()
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for QueryEventCount {
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() == "sqlx::query" {
            *self.0.lock().unwrap() += 1;
        }
    }
}

/// Nested "just works" when both parent and child snapshot: a nested
/// `find_by_id` still issues exactly one SQL statement for the whole tree.
#[tokio::test]
async fn nested_both_snapshot_one_statement() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let repo = BothSites::new(pool);

    let site_id = SiteId::new();
    let meter_id = MeterId::new();
    let mut site = repo
        .create(NewSite::builder().id(site_id).build().unwrap())
        .await?;
    site.add_meter(new_meter(meter_id, site_id, "m"));
    repo.update(&mut site).await?;

    // Cross the child's own snapshot threshold (tail_len() >= 4) the same
    // way a flat repo would.
    for v in 1..=5i64 {
        let _ = site.meters.get_persisted_mut(&meter_id).unwrap().record(v);
        repo.update(&mut site).await?;
    }

    let counter = QueryEventCount::default();
    let subscriber = tracing_subscriber::registry().with(counter.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let loaded = repo.find_by_id(site_id).await?;

    assert_eq!(
        counter.get(),
        1,
        "a nested load with both parent and child snapshotting must be one statement"
    );
    let meter = loaded.meters.get_persisted(&meter_id).unwrap();
    assert!(meter.has_snapshot());
    assert_eq!(meter.count(), 5);

    Ok(())
}

/// Refresh-on-write over a nested batch: three children snapshot, all three
/// fingerprints go stale, only one child is mutated — the parent `update`
/// still refreshes every stale child in the same call, not just the mutated
/// one, because `update_all` (which the nested phase routes through) checks
/// every child it is handed, not only the dirty ones.
#[tokio::test]
async fn nested_update_refreshes_every_stale_child_not_just_the_mutated_one() -> anyhow::Result<()>
{
    let pool = init_pool().await?;
    let repo = BothSites::new(pool.clone());

    let site_id = SiteId::new();
    let mut site = repo
        .create(NewSite::builder().id(site_id).build().unwrap())
        .await?;

    let meter_ids: Vec<MeterId> = (0..3).map(|_| MeterId::new()).collect();
    for &id in &meter_ids {
        site.add_meter(new_meter(id, site_id, "m"));
    }
    repo.update(&mut site).await?;

    // Cross every child's own snapshot threshold.
    for &id in &meter_ids {
        for v in 1..=5i64 {
            let _ = site.meters.get_persisted_mut(&id).unwrap().record(v);
        }
    }
    repo.update(&mut site).await?;
    for &id in &meter_ids {
        assert!(site.meters.get_persisted(&id).unwrap().has_snapshot());
    }

    // Every child's snapshot goes stale.
    for &id in &meter_ids {
        sqlx::query!(
            "UPDATE meter_snapshots SET fingerprint = fingerprint + 1 WHERE id = $1",
            id as MeterId
        )
        .execute(&pool)
        .await?;
    }

    let mut reloaded = repo.find_by_id(site_id).await?;
    for &id in &meter_ids {
        assert!(!reloaded.meters.get_persisted(&id).unwrap().has_snapshot());
    }

    // Mutate only the first child, then update the parent.
    let _ = reloaded
        .meters
        .get_persisted_mut(&meter_ids[0])
        .unwrap()
        .record(6);
    repo.update(&mut reloaded).await?;

    // All three children — not just the mutated one — are fresh again.
    for &id in &meter_ids {
        assert!(
            reloaded.meters.get_persisted(&id).unwrap().has_snapshot(),
            "child {id} should have been refreshed even though it staged no events"
        );
    }

    let final_load = repo.find_by_id(site_id).await?;
    for &id in &meter_ids {
        assert!(final_load.meters.get_persisted(&id).unwrap().has_snapshot());
    }
    assert_eq!(
        final_load
            .meters
            .get_persisted(&meter_ids[0])
            .unwrap()
            .count(),
        6
    );
    for &id in &meter_ids[1..] {
        assert_eq!(final_load.meters.get_persisted(&id).unwrap().count(), 5);
    }

    Ok(())
}
