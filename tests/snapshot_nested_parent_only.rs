#![cfg(feature = "instrument")]

mod entities;
mod helpers;

use std::sync::{Arc, Mutex};

use entities::{meter::*, site::*};
use es_entity::*;
use helpers::init_pool;
use sqlx::PgPool;
use tracing_subscriber::layer::SubscriberExt;

/// Parent (`SiteOverPlainMeters`) snapshots, child (`PlainMeter`) does not.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "SiteOverPlainMeters",
    id = "SiteId",
    event = "SiteEvent",
    tbl = "sites",
    events_tbl = "site_events",
    snapshot,
    snapshot_tbl = "site_snapshots"
)]
pub struct ParentOnlySites {
    pool: PgPool,

    #[es_repo(nested)]
    meters: ParentOnlyMeters,
}

impl ParentOnlySites {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            meters: ParentOnlyMeters::new(pool),
        }
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "PlainMeter",
    id = "MeterId",
    event = "MeterEvent",
    tbl = "meters",
    events_tbl = "meter_events",
    columns(
        site_id(ty = "SiteId", update(persist = false), parent),
        label(ty = "String")
    )
)]
pub struct ParentOnlyMeters {
    pool: PgPool,
}

impl ParentOnlyMeters {
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

/// Test 10 — nested "just works" when only the parent snapshots: a nested
/// `find_by_id` still issues exactly one SQL statement for the whole tree.
#[tokio::test]
async fn nested_parent_only_snapshot_one_statement() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let repo = ParentOnlySites::new(pool);

    let site_id = SiteId::new();
    let meter_id = MeterId::new();
    let mut site = repo
        .create(NewSite::builder().id(site_id).build().unwrap())
        .await?;
    site.add_meter(new_meter(meter_id, site_id, "m"));
    repo.update(&mut site).await?;

    let counter = QueryEventCount::default();
    let subscriber = tracing_subscriber::registry().with(counter.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let loaded = repo.find_by_id(site_id).await?;

    assert_eq!(
        counter.get(),
        1,
        "a nested load with only the parent snapshotting must be one statement"
    );
    assert!(loaded.meters.get_persisted(&meter_id).is_some());

    Ok(())
}
