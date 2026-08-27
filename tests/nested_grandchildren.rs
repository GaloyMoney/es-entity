#![cfg(feature = "instrument")]
//! Proves nested batching composes to arbitrary depth: a three-level
//! parent -> child -> grandchild hierarchy, where the grandchild phase
//! (gathered *across every child of every parent* in one
//! `update_all_in_op` call on the top-level parent repo) collapses to a
//! single statement per grandchild repo fn, exactly like the one-level case
//! in `nested_batch_statement_count.rs`.
//!
//! The mechanism has no depth-specific code: `create_nested_*_in_op` /
//! `update_nested_*_in_op` always take `&mut [&mut P]` and the middle
//! (`GcChild`) repo's own `update_all_mut_in_op` runs its *own* nested phase
//! the same way any other bulk fn does — so this is exercising the exact
//! same generated code path as the one-level test, just nested one level
//! deeper.

mod helpers;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use derive_builder::Builder;
use es_entity::*;
use helpers::init_pool;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;
use tracing_subscriber::layer::SubscriberExt;

es_entity::entity_id! { GcParentId, GcChildId, GcGrandchildId }

// ─── Grandchild ─────────────────────────────────────────────────────────
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "GcGrandchildId")]
pub enum GcGrandchildEvent {
    Initialized {
        id: GcGrandchildId,
        child_id: GcChildId,
        note: String,
    },
    NoteUpdated {
        note: String,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct GcGrandchild {
    pub id: GcGrandchildId,
    pub child_id: GcChildId,
    pub note: String,
    events: EntityEvents<GcGrandchildEvent>,
}

impl GcGrandchild {
    pub fn update_note(&mut self, note: impl Into<String>) {
        let note = note.into();
        self.note = note.clone();
        self.events.push(GcGrandchildEvent::NoteUpdated { note });
    }
}

impl TryFromEvents<GcGrandchildEvent> for GcGrandchild {
    fn try_from_events(
        events: EntityEvents<GcGrandchildEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let mut builder = GcGrandchildBuilder::default();
        for event in events.iter_all() {
            match event {
                GcGrandchildEvent::Initialized { id, child_id, note } => {
                    builder = builder.id(*id).child_id(*child_id).note(note.clone());
                }
                GcGrandchildEvent::NoteUpdated { note } => {
                    builder = builder.note(note.clone());
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewGcGrandchild {
    pub id: GcGrandchildId,
    pub child_id: GcChildId,
    #[builder(setter(into))]
    pub note: String,
}

impl NewGcGrandchild {
    pub fn builder() -> NewGcGrandchildBuilder {
        NewGcGrandchildBuilder::default()
    }
}

impl IntoEvents<GcGrandchildEvent> for NewGcGrandchild {
    fn into_events(self) -> EntityEvents<GcGrandchildEvent> {
        EntityEvents::init(
            self.id,
            vec![GcGrandchildEvent::Initialized {
                id: self.id,
                child_id: self.child_id,
                note: self.note,
            }],
        )
    }
}

// ─── Child ──────────────────────────────────────────────────────────────
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "GcChildId")]
pub enum GcChildEvent {
    Initialized {
        id: GcChildId,
        parent_id: GcParentId,
    },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct GcChild {
    pub id: GcChildId,
    pub parent_id: GcParentId,
    events: EntityEvents<GcChildEvent>,

    #[es_entity(nested)]
    #[builder(default)]
    grandchildren: Nested<GcGrandchild>,
}

impl GcChild {
    pub fn add_grandchild(&mut self, new: NewGcGrandchild) {
        self.grandchildren.add_new(new);
    }

    pub fn update_grandchild_note(&mut self, note: impl Into<String>) {
        let note = note.into();
        let child = self
            .grandchildren
            .iter_persisted_mut()
            .next()
            .expect("child has a persisted grandchild");
        child.update_note(note);
    }

    pub fn n_grandchildren(&self) -> usize {
        self.grandchildren.len_persisted()
    }
}

impl TryFromEvents<GcChildEvent> for GcChild {
    fn try_from_events(events: EntityEvents<GcChildEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = GcChildBuilder::default();
        for event in events.iter_all() {
            match event {
                GcChildEvent::Initialized { id, parent_id } => {
                    builder = builder.id(*id).parent_id(*parent_id);
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewGcChild {
    pub id: GcChildId,
    pub parent_id: GcParentId,
}

impl IntoEvents<GcChildEvent> for NewGcChild {
    fn into_events(self) -> EntityEvents<GcChildEvent> {
        EntityEvents::init(
            self.id,
            vec![GcChildEvent::Initialized {
                id: self.id,
                parent_id: self.parent_id,
            }],
        )
    }
}

// ─── Parent ─────────────────────────────────────────────────────────────
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "GcParentId")]
pub enum GcParentEvent {
    Initialized { id: GcParentId },
}

#[derive(EsEntity, Builder)]
#[builder(pattern = "owned", build_fn(error = "EntityHydrationError"))]
pub struct GcParent {
    pub id: GcParentId,
    events: EntityEvents<GcParentEvent>,

    #[es_entity(nested)]
    #[builder(default)]
    children: Nested<GcChild>,
}

impl GcParent {
    pub fn add_child(&mut self, new: NewGcChild) {
        self.children.add_new(new);
    }

    pub fn persisted_children_mut(&mut self) -> impl Iterator<Item = &mut GcChild> {
        self.children.iter_persisted_mut()
    }
}

impl TryFromEvents<GcParentEvent> for GcParent {
    fn try_from_events(events: EntityEvents<GcParentEvent>) -> Result<Self, EntityHydrationError> {
        let mut builder = GcParentBuilder::default();
        for event in events.iter_all() {
            match event {
                GcParentEvent::Initialized { id } => {
                    builder = builder.id(*id);
                }
            }
        }
        builder.events(events).build()
    }
}

#[derive(Debug, Clone, Builder)]
pub struct NewGcParent {
    pub id: GcParentId,
}

impl IntoEvents<GcParentEvent> for NewGcParent {
    fn into_events(self) -> EntityEvents<GcParentEvent> {
        EntityEvents::init(self.id, vec![GcParentEvent::Initialized { id: self.id }])
    }
}

// ─── Repos ──────────────────────────────────────────────────────────────
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "GcGrandchild",
    tbl = "gc_grandchildren",
    events_tbl = "gc_grandchild_events",
    delete = "soft",
    columns(child_id(ty = "GcChildId", update(persist = false), parent))
)]
pub struct GcGrandchildren {
    pool: PgPool,
}

impl GcGrandchildren {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "GcChild",
    tbl = "gc_children",
    events_tbl = "gc_child_events",
    delete = "soft",
    columns(parent_id(ty = "GcParentId", update(persist = false), parent))
)]
pub struct GcChildren {
    pool: PgPool,

    #[es_repo(nested)]
    grandchildren: GcGrandchildren,
}

impl GcChildren {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            grandchildren: GcGrandchildren::new(pool),
        }
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "GcParent",
    tbl = "gc_parents",
    events_tbl = "gc_parent_events",
    delete = "soft"
)]
pub struct GcParents {
    pool: PgPool,

    #[es_repo(nested)]
    children: GcChildren,
}

impl GcParents {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool: pool.clone(),
            children: GcChildren::new(pool),
        }
    }
}

// ─── Span counting ──────────────────────────────────────────────────────
#[derive(Clone, Default)]
struct SpanCounts(Arc<Mutex<HashMap<String, usize>>>);

impl SpanCounts {
    fn count(&self, name: &str) -> usize {
        self.0.lock().unwrap().get(name).copied().unwrap_or(0)
    }
}

impl<S: tracing::Subscriber> tracing_subscriber::Layer<S> for SpanCounts {
    fn on_new_span(
        &self,
        attrs: &tracing::span::Attributes<'_>,
        _id: &tracing::span::Id,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        *self
            .0
            .lock()
            .unwrap()
            .entry(attrs.metadata().name().to_string())
            .or_insert(0) += 1;
    }
}

/// 2 parents x 2 children x 1 persisted grandchild each: mutating every
/// grandchild's note and adding a new grandchild under every child, then
/// flushing the whole tree through a single top-level `update_all_in_op`
/// call, should collapse the grandchild phase to one `update_all_mut` call
/// and one `create_all` call on the grandchild repo — not one per child
/// (there are 4 children) and not one per parent (there are 2).
#[tokio::test]
async fn grandchild_batching_composes_across_the_whole_tree() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let parents_repo = GcParents::new(pool.clone());
    let children_repo = GcChildren::new(pool);

    const N_PARENTS: usize = 2;
    const N_CHILDREN_PER_PARENT: usize = 2;

    let mut parent_ids = Vec::new();
    for _ in 0..N_PARENTS {
        let parent_id = GcParentId::new();
        let mut parent = parents_repo
            .create(NewGcParentBuilder::default().id(parent_id).build()?)
            .await?;
        for _ in 0..N_CHILDREN_PER_PARENT {
            let child_id = GcChildId::new();
            parent.add_child(
                NewGcChildBuilder::default()
                    .id(child_id)
                    .parent_id(parent_id)
                    .build()?,
            );
        }
        parents_repo.update(&mut parent).await?;
        parent_ids.push(parent_id);
    }

    // Give every persisted child one persisted grandchild, via the child
    // repo directly (children were just created above).
    let mut all_children = children_repo
        .find_all::<GcChild>(
            &sqlx::query_scalar!(
                "SELECT id AS \"id: GcChildId\" FROM gc_children WHERE parent_id = ANY($1) ORDER BY id",
                &parent_ids as &[GcParentId],
            )
            .fetch_all(&children_repo.pool().clone())
            .await?,
        )
        .await?;
    let mut children_vec: Vec<GcChild> = all_children.drain().map(|(_, c)| c).collect();
    assert_eq!(children_vec.len(), N_PARENTS * N_CHILDREN_PER_PARENT);
    for child in children_vec.iter_mut() {
        child.add_grandchild(
            NewGcGrandchild::builder()
                .id(GcGrandchildId::new())
                .child_id(child.id)
                .note("initial")
                .build()?,
        );
    }
    children_repo.update_all(&mut children_vec).await?;

    // Reload the whole tree, mutate every grandchild's note and add a new
    // grandchild under every child, then flush it all through one
    // top-level `update_all_in_op` call.
    let mut loaded = parents_repo.find_all::<GcParent>(&parent_ids).await?;
    let mut batch: Vec<GcParent> = parent_ids
        .iter()
        .map(|id| loaded.remove(id).expect("parent was loaded"))
        .collect();

    for parent in batch.iter_mut() {
        for child in parent.persisted_children_mut() {
            assert_eq!(
                child.n_grandchildren(),
                1,
                "child should have its seeded grandchild"
            );
            child.update_grandchild_note("updated");
            child.add_grandchild(
                NewGcGrandchild::builder()
                    .id(GcGrandchildId::new())
                    .child_id(child.id)
                    .note("new")
                    .build()?,
            );
        }
    }

    let counts = SpanCounts::default();
    let subscriber = tracing_subscriber::registry().with(counts.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    parents_repo.update_all(&mut batch).await?;

    assert_eq!(
        counts.count("gc_grandchildren.update_all_mut"),
        1,
        "grandchild note updates across every child of every parent should collapse into one \
         gc_grandchildren.update_all_mut call"
    );
    assert_eq!(
        counts.count("gc_grandchildren.create_all"),
        1,
        "new grandchildren across every child of every parent should collapse into one \
         gc_grandchildren.create_all call"
    );
    // The child level itself batches too: one update_all_mut for the
    // (unchanged, but visited) persisted children, none of the old
    // per-entity fallbacks.
    assert_eq!(counts.count("gc_children.update"), 0);
    assert_eq!(counts.count("gc_grandchildren.update"), 0);

    // Functional correctness, not just statement counts.
    let reloaded = parents_repo.find_all::<GcParent>(&parent_ids).await?;
    for (_, mut parent) in reloaded {
        for child in parent.persisted_children_mut() {
            assert_eq!(
                child.n_grandchildren(),
                2,
                "one updated + one new grandchild"
            );
        }
    }

    Ok(())
}

/// A **grandchild-only** mutation must bump the root's aggregate version.
///
/// This is the case a depth-1 pending-work check silently misses: the root has
/// no new events and its children are all clean, so only a recursive walk sees
/// that a grandchild is dirty. Missing it would let a grandchild write commit
/// without touching the aggregate clock, reopening write skew one level down.
///
/// Revert-to-red: make the generated pending-work check non-recursive (drop the
/// `has_pending_nested_work` delegation in `nested.rs`) and this fails
/// deterministically — the version stays put and the stale writer succeeds.
#[tokio::test]
async fn grandchild_only_mutation_bumps_the_root_version() -> anyhow::Result<()> {
    let pool = init_pool().await?;
    let parents_repo = GcParents::new(pool.clone());
    let children_repo = GcChildren::new(pool.clone());

    // parent -> child -> grandchild, all persisted.
    let parent_id = GcParentId::new();
    let child_id = GcChildId::new();
    let mut parent = parents_repo
        .create(NewGcParentBuilder::default().id(parent_id).build()?)
        .await?;
    parent.add_child(
        NewGcChildBuilder::default()
            .id(child_id)
            .parent_id(parent_id)
            .build()?,
    );
    parents_repo.update(&mut parent).await?;

    let mut child = children_repo.find_by_id(child_id).await?;
    child.add_grandchild(
        NewGcGrandchild::builder()
            .id(GcGrandchildId::new())
            .child_id(child_id)
            .note("initial")
            .build()?,
    );
    children_repo.update(&mut child).await?;

    let version_before = sqlx::query!(
        "SELECT version FROM gc_parents WHERE id = $1",
        parent_id as GcParentId
    )
    .fetch_one(&pool)
    .await?
    .version;

    // Touch ONLY the grandchild. Root stream clean, child stream clean.
    let mut parent = parents_repo.find_by_id(parent_id).await?;
    for child in parent.persisted_children_mut() {
        child.update_grandchild_note("updated");
    }
    parents_repo.update(&mut parent).await?;

    let version_after = sqlx::query!(
        "SELECT version FROM gc_parents WHERE id = $1",
        parent_id as GcParentId
    )
    .fetch_one(&pool)
    .await?
    .version;

    assert_eq!(
        version_after,
        version_before + 1,
        "a grandchild-only mutation must still bump the root's aggregate version"
    );

    // And the bump is load-bearing: a writer holding the pre-mutation version
    // is now rejected.
    let mut stale = parents_repo.find_by_id(parent_id).await?;
    let mut fresh = parents_repo.find_by_id(parent_id).await?;
    for child in fresh.persisted_children_mut() {
        child.update_grandchild_note("winner");
    }
    parents_repo.update(&mut fresh).await?;

    for child in stale.persisted_children_mut() {
        child.update_grandchild_note("loser");
    }
    let err = parents_repo
        .update(&mut stale)
        .await
        .expect_err("stale grandchild writer must be rejected");
    assert!(
        err.was_concurrent_modification(),
        "expected ConcurrentModification, got: {err}"
    );

    Ok(())
}
