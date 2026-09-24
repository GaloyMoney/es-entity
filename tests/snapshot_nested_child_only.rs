#![cfg(feature = "instrument")]

mod entities;
mod helpers;

use std::sync::{Arc, Mutex};

use entities::{meter::*, site::*};
use es_entity::*;
use helpers::init_pool;
use sqlx::PgPool;
use tracing_subscriber::layer::SubscriberExt;

/// Child (`Meter`) snapshots, parent (`PlainSiteOverMeters`) does not.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "PlainSiteOverMeters",
    id = "SiteId",
    event = "SiteEvent",
    tbl = "sites",
    events_tbl = "site_events"
)]
pub struct ChildOnlySites {
    pool: PgPool,

    #[es_repo(nested)]
    meters: ChildOnlyMeters,
}

impl ChildOnlySites {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            meters: ChildOnlyMeters::new(pool),
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
pub struct ChildOnlyMeters {
    pool: PgPool,
}

impl ChildOnlyMeters {
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

/// Nested "just works" when only the child snapshots: a nested `find_by_id`
/// still issues exactly one SQL statement for the whole tree.
#[tokio::test]
async fn nested_child_only_snapshot_one_statement() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let repo = ChildOnlySites::new(pool);

    let site_id = SiteId::new();
    let meter_id = MeterId::new();
    let mut site = repo
        .create(NewSite::builder().id(site_id).build().unwrap())
        .await?;
    site.add_meter(new_meter(meter_id, site_id, "m"));
    repo.update(&mut site).await?;

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
        "a nested load with only the child snapshotting must be one statement"
    );
    let meter = loaded.meters.get_persisted(&meter_id).unwrap();
    assert!(meter.has_snapshot());
    assert_eq!(meter.count(), 5);

    Ok(())
}
