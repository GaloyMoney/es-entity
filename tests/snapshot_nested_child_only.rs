#![cfg(feature = "instrument")]

mod entities;
mod helpers;

use std::sync::{Arc, Mutex};

use entities::{meter::*, site::*};
use es_entity::{db::Pool, *};
use helpers::init_pool;
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
    pool: Pool,

    #[es_repo(nested, entity = "Meter")]
    meters: ChildOnlyMeters,
}

impl ChildOnlySites {
    pub fn new(pool: Pool) -> Self {
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
    pool: Pool,
}

impl ChildOnlyMeters {
    pub fn new(pool: Pool) -> Self {
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
// Postgres-only: counts `sqlx::query` events on a thread-local subscriber;
// sqlx-sqlite runs statements on its worker thread, so none are observed.
#[cfg(feature = "postgres")]
#[tokio::test]
async fn nested_child_only_snapshot_one_statement() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let repo = ChildOnlySites::new(pool);

    let mut site = repo.create(NewSite::builder().build()).await?;
    let site_id = site.id;
    site.add_meter(new_meter(site_id, "m"));
    repo.update(&mut site).await?;
    let meter_id = site.meters.iter_persisted().next().unwrap().id;

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
