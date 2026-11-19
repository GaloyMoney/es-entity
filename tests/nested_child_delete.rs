//! Deleting nested children: `Nested<T, SoftDelete>` + `delete_persisted`, and
//! the child repo's `delete_all_in_op`.
//!
//! A three-level fixture (parent -> child -> grandchild), plus a forgettable
//! child and a child repo with a `post_persist_hook`. The fixture is
//! self-contained on purpose: the shared `Order` fixture nests a non-soft
//! `OrderItem` repo in `tests/from_async_trait.rs`.

mod helpers;

use std::sync::{Arc, Mutex};

use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! { NcdParentId, NcdChildId, NcdGrandchildId, NcdSecretId, NcdHookedId }

// ─── Grandchild ─────────────────────────────────────────────────────────
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "NcdGrandchildId")]
pub enum NcdGrandchildEvent {
    Initialized {
        id: NcdGrandchildId,
        child_id: NcdChildId,
    },
}

#[derive(EsEntity)]
pub struct NcdGrandchild {
    pub id: NcdGrandchildId,
    pub child_id: NcdChildId,
    events: EntityEvents<NcdGrandchildEvent>,
}

impl TryFromEvents<NcdGrandchildEvent> for NcdGrandchild {
    fn try_from_events(
        events: EntityEvents<NcdGrandchildEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let mut h = NcdGrandchild::hydrator();
        for event in events.iter_all() {
            let NcdGrandchildEvent::Initialized { id, child_id } = event;
            h.id(*id).child_id(*child_id);
        }
        h.build(events)
    }
}

#[derive(Debug, Clone, bon::Builder)]
pub struct NewNcdGrandchild {
    pub child_id: NcdChildId,
}

impl IntoEvents<NcdGrandchildEvent> for NewNcdGrandchild {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<NcdGrandchildEvent, FromRepo>,
    ) -> EntityEvents<NcdGrandchildEvent> {
        let id = builder.id();
        builder.initial_events([NcdGrandchildEvent::Initialized {
            id,
            child_id: self.child_id,
        }])
    }
}

// ─── Child ──────────────────────────────────────────────────────────────
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "NcdChildId")]
pub enum NcdChildEvent {
    Initialized {
        id: NcdChildId,
        parent_id: NcdParentId,
        label: String,
    },
    LabelChanged {
        label: String,
    },
    Retired,
}

#[derive(EsEntity)]
pub struct NcdChild {
    pub id: NcdChildId,
    pub parent_id: NcdParentId,
    pub label: String,
    events: EntityEvents<NcdChildEvent>,

    #[es_entity(nested)]
    grandchildren: Nested<NcdGrandchild>,
}

impl NcdChild {
    fn set_label(&mut self, label: &str) {
        self.label = label.to_string();
        self.events.push(NcdChildEvent::LabelChanged {
            label: label.to_string(),
        });
    }

    fn retire(&mut self) {
        self.events.push(NcdChildEvent::Retired);
    }

    fn add_grandchild(&mut self) {
        self.grandchildren
            .add_new(NewNcdGrandchild::builder().child_id(self.id).build());
    }
}

impl TryFromEvents<NcdChildEvent> for NcdChild {
    fn try_from_events(events: EntityEvents<NcdChildEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = NcdChild::hydrator();
        for event in events.iter_all() {
            match event {
                NcdChildEvent::Initialized {
                    id,
                    parent_id,
                    label,
                } => {
                    h.id(*id).parent_id(*parent_id).label(label.clone());
                }
                NcdChildEvent::LabelChanged { label } => {
                    h.label(label.clone());
                }
                NcdChildEvent::Retired => {}
            }
        }
        h.build(events)
    }
}

#[derive(Debug, Clone, bon::Builder)]
pub struct NewNcdChild {
    pub parent_id: NcdParentId,
    #[builder(into)]
    pub label: String,
}

impl IntoEvents<NcdChildEvent> for NewNcdChild {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<NcdChildEvent, FromRepo>,
    ) -> EntityEvents<NcdChildEvent> {
        let id = builder.id();
        builder.initial_events([NcdChildEvent::Initialized {
            id,
            parent_id: self.parent_id,
            label: self.label,
        }])
    }
}

// ─── Forgettable child ──────────────────────────────────────────────────
#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "NcdSecretId")]
pub enum NcdSecretEvent {
    Initialized {
        id: NcdSecretId,
        parent_id: NcdParentId,
        name: Forgettable<String>,
        email: Forgettable<String>,
    },
}

#[derive(EsEntity)]
pub struct NcdSecret {
    pub id: NcdSecretId,
    pub parent_id: NcdParentId,
    pub name: Forgettable<String>,
    pub email: Forgettable<String>,
    events: EntityEvents<NcdSecretEvent>,
}

impl TryFromEvents<NcdSecretEvent> for NcdSecret {
    fn try_from_events(events: EntityEvents<NcdSecretEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = NcdSecret::hydrator();
        for event in events.iter_all() {
            let NcdSecretEvent::Initialized {
                id,
                parent_id,
                name,
                email,
            } = event;
            h.id(*id)
                .parent_id(*parent_id)
                .name(name.clone())
                .email(email.clone());
        }
        h.build(events)
    }
}

#[derive(Debug, bon::Builder)]
pub struct NewNcdSecret {
    pub parent_id: NcdParentId,
    #[builder(into)]
    pub name: String,
    #[builder(into)]
    pub email: String,
}

impl IntoEvents<NcdSecretEvent> for NewNcdSecret {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<NcdSecretEvent, FromRepo>,
    ) -> EntityEvents<NcdSecretEvent> {
        let id = builder.id();
        builder.initial_events([NcdSecretEvent::Initialized {
            id,
            parent_id: self.parent_id,
            name: Forgettable::new(self.name),
            email: Forgettable::new(self.email),
        }])
    }
}

// ─── Child with a post-persist hook ─────────────────────────────────────
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "NcdHookedId")]
pub enum NcdHookedEvent {
    Initialized {
        id: NcdHookedId,
        parent_id: NcdParentId,
    },
    Closed,
}

#[derive(EsEntity)]
pub struct NcdHooked {
    pub id: NcdHookedId,
    pub parent_id: NcdParentId,
    events: EntityEvents<NcdHookedEvent>,
}

impl NcdHooked {
    fn close(&mut self) {
        self.events.push(NcdHookedEvent::Closed);
    }
}

impl TryFromEvents<NcdHookedEvent> for NcdHooked {
    fn try_from_events(events: EntityEvents<NcdHookedEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = NcdHooked::hydrator();
        for event in events.iter_all() {
            if let NcdHookedEvent::Initialized { id, parent_id } = event {
                h.id(*id).parent_id(*parent_id);
            }
        }
        h.build(events)
    }
}

#[derive(Debug, Clone, bon::Builder)]
pub struct NewNcdHooked {
    pub parent_id: NcdParentId,
}

impl IntoEvents<NcdHookedEvent> for NewNcdHooked {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<NcdHookedEvent, FromRepo>,
    ) -> EntityEvents<NcdHookedEvent> {
        let id = builder.id();
        builder.initial_events([NcdHookedEvent::Initialized {
            id,
            parent_id: self.parent_id,
        }])
    }
}

// ─── Parent ─────────────────────────────────────────────────────────────
#[derive(EsEvent, Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "NcdParentId")]
pub enum NcdParentEvent {
    Initialized { id: NcdParentId },
}

#[derive(EsEntity)]
pub struct NcdParent {
    pub id: NcdParentId,
    events: EntityEvents<NcdParentEvent>,

    #[es_entity(nested)]
    children: Nested<NcdChild, SoftDelete>,
    #[es_entity(nested)]
    secrets: Nested<NcdSecret, SoftDelete>,
    #[es_entity(nested)]
    hooked: Nested<NcdHooked, SoftDelete>,
}

impl NcdParent {
    fn add_child(&mut self, label: &str) {
        self.children.add_new(
            NewNcdChild::builder()
                .parent_id(self.id)
                .label(label)
                .build(),
        );
    }

    fn child_ids(&self) -> Vec<NcdChildId> {
        let mut ids: Vec<_> = self.children.iter_persisted().map(|c| c.id).collect();
        ids.sort_by_key(|id| id.to_string());
        ids
    }

    fn child_by_label(&self, label: &str) -> NcdChildId {
        self.children
            .find_persisted(|c| c.label == label)
            .expect("child with label")
            .id
    }

    /// Removes the child and pushes a final event on it.
    fn remove_child(&mut self, id: &NcdChildId) -> bool {
        match self.children.delete_persisted(id) {
            Some(child) => {
                child.retire();
                true
            }
            None => false,
        }
    }

    fn remove_secret(&mut self, id: &NcdSecretId) {
        self.secrets.delete_persisted(id);
    }

    fn remove_hooked(&mut self, id: &NcdHookedId) {
        self.hooked
            .delete_persisted(id)
            .expect("persisted hooked child")
            .close();
    }
}

impl TryFromEvents<NcdParentEvent> for NcdParent {
    fn try_from_events(events: EntityEvents<NcdParentEvent>) -> Result<Self, EntityHydrationError> {
        let mut h = NcdParent::hydrator();
        for event in events.iter_all() {
            let NcdParentEvent::Initialized { id } = event;
            h.id(*id);
        }
        h.build(events)
    }
}

#[derive(Debug, Clone, bon::Builder)]
pub struct NewNcdParent {}

impl IntoEvents<NcdParentEvent> for NewNcdParent {
    type IdSource = FromRepo;

    fn into_events(
        self,
        builder: EntityEventsBuilder<NcdParentEvent, FromRepo>,
    ) -> EntityEvents<NcdParentEvent> {
        let id = builder.id();
        builder.initial_events([NcdParentEvent::Initialized { id }])
    }
}

// ─── Repos ──────────────────────────────────────────────────────────────
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "NcdGrandchild",
    tbl = "ncd_grandchildren",
    events_tbl = "ncd_grandchild_events",
    delete = "soft",
    columns(child_id(ty = "NcdChildId", update(persist = false), parent))
)]
pub struct NcdGrandchildren {
    pool: PgPool,
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "NcdChild",
    tbl = "ncd_children",
    events_tbl = "ncd_child_events",
    delete = "soft",
    columns(
        parent_id(ty = "NcdParentId", update(persist = false), parent),
        label(ty = "String")
    )
)]
pub struct NcdChildren {
    pool: PgPool,

    #[es_repo(nested)]
    grandchildren: NcdGrandchildren,
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "NcdSecret",
    tbl = "ncd_secrets",
    events_tbl = "ncd_secret_events",
    forgettable,
    delete = "soft",
    columns(
        parent_id(ty = "NcdParentId", update(persist = false), parent),
        email(ty = "Forgettable<String>")
    )
)]
pub struct NcdSecrets {
    pool: PgPool,
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "NcdHooked",
    tbl = "ncd_hooked",
    events_tbl = "ncd_hooked_events",
    delete = "soft",
    columns(parent_id(ty = "NcdParentId", update(persist = false), parent)),
    post_persist_hook = "on_persist"
)]
pub struct NcdHookedRepo {
    pool: PgPool,
    /// Number of events the hook saw, one entry per hook call.
    seen: Arc<Mutex<Vec<usize>>>,
}

impl NcdHookedRepo {
    async fn on_persist<OP: es_entity::AtomicOperation>(
        &self,
        _op: &mut OP,
        _entity: &NcdHooked,
        new_events: es_entity::LastPersisted<'_, NcdHookedEvent>,
    ) -> Result<(), errlanes::Fault<errlanes::lanes!(Transient, Fatal)>> {
        self.seen.lock().unwrap().push(new_events.len());
        Ok(())
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "NcdParent",
    tbl = "ncd_parents",
    events_tbl = "ncd_parent_events",
    delete = "soft"
)]
pub struct NcdParents {
    pool: PgPool,

    #[es_repo(nested)]
    children: NcdChildren,
    #[es_repo(nested)]
    secrets: NcdSecrets,
    #[es_repo(nested)]
    hooked: NcdHookedRepo,
}

struct Fixture {
    pool: PgPool,
    parents: NcdParents,
    hook_seen: Arc<Mutex<Vec<usize>>>,
}

async fn fixture() -> anyhow::Result<Fixture> {
    let pool = helpers::init_pool().await?;
    let hook_seen = Arc::new(Mutex::new(Vec::new()));
    let parents = NcdParents {
        pool: pool.clone(),
        children: NcdChildren {
            pool: pool.clone(),
            grandchildren: NcdGrandchildren { pool: pool.clone() },
        },
        secrets: NcdSecrets { pool: pool.clone() },
        hooked: NcdHookedRepo {
            pool: pool.clone(),
            seen: hook_seen.clone(),
        },
    };
    Ok(Fixture {
        pool,
        parents,
        hook_seen,
    })
}

async fn parent_with_children(fx: &Fixture, labels: &[&str]) -> anyhow::Result<NcdParent> {
    let mut parent = fx.parents.create(NewNcdParent::builder().build()).await?;
    for label in labels {
        parent.add_child(label);
    }
    fx.parents.update(&mut parent).await?;
    Ok(fx.parents.find_by_id(parent.id).await?)
}

async fn child_deleted(pool: &PgPool, id: NcdChildId) -> anyhow::Result<bool> {
    Ok(
        sqlx::query_scalar::<_, bool>("SELECT deleted FROM ncd_children WHERE id = $1")
            .bind(id)
            .fetch_one(pool)
            .await?,
    )
}

async fn child_event_types(pool: &PgPool, id: NcdChildId) -> anyhow::Result<Vec<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT event_type FROM ncd_child_events WHERE id = $1 ORDER BY sequence",
    )
    .bind(id)
    .fetch_all(pool)
    .await?)
}

// ─── Runtime unit tests for the container ───────────────────────────────

#[tokio::test]
async fn delete_persisted_moves_the_child_and_take_removed_drains() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let mut parent = parent_with_children(&fx, &["a"]).await?;
    let id = parent.child_ids()[0];
    let nested = &mut parent.children;
    assert_eq!(nested.len_persisted(), 1);

    assert!(nested.delete_persisted(&NcdChildId::new()).is_none());
    assert_eq!(nested.len_removed(), 0);

    assert!(nested.delete_persisted(&id).is_some());
    assert_eq!(nested.len_persisted(), 0);
    assert_eq!(nested.len_removed(), 1);
    assert_eq!(nested.iter_removed().count(), 1);
    // A second delete of the same id finds nothing.
    assert!(nested.delete_persisted(&id).is_none());

    assert_eq!(nested.take_removed().len(), 1);
    assert_eq!(nested.len_removed(), 0);
    Ok(())
}

// ─── Integration ────────────────────────────────────────────────────────

#[tokio::test]
async fn delete_one_of_three_children() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let mut parent = parent_with_children(&fx, &["a", "b", "c"]).await?;
    assert_eq!(parent.children.len_persisted(), 3);
    let victim = parent.child_by_label("b");

    assert!(parent.remove_child(&victim));
    assert_eq!(parent.children.len_removed(), 1);
    fx.parents.update(&mut parent).await?;

    let reloaded = fx.parents.find_by_id(parent.id).await?;
    assert_eq!(reloaded.children.len_persisted(), 2);
    assert!(reloaded.children.get_persisted(&victim).is_none());

    let with_deleted = fx.parents.find_by_id_include_deleted(parent.id).await?;
    assert_eq!(with_deleted.children.len_persisted(), 3);
    assert!(child_deleted(&fx.pool, victim).await?);
    Ok(())
}

#[tokio::test]
async fn staged_event_on_a_removed_child_is_persisted() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let mut parent = parent_with_children(&fx, &["a", "b"]).await?;
    let victim = parent.child_by_label("a");

    // Mutate the child, then delete it in the same command.
    parent
        .children
        .get_persisted_mut(&victim)
        .unwrap()
        .set_label("renamed");
    assert!(parent.remove_child(&victim));
    fx.parents.update(&mut parent).await?;

    assert_eq!(
        child_event_types(&fx.pool, victim).await?,
        vec!["initialized", "label_changed", "retired"]
    );
    // The index column written by the delete carries the live value.
    let label: String = sqlx::query_scalar("SELECT label FROM ncd_children WHERE id = $1")
        .bind(victim)
        .fetch_one(&fx.pool)
        .await?;
    assert_eq!(label, "renamed");
    assert!(child_deleted(&fx.pool, victim).await?);
    Ok(())
}

#[tokio::test]
async fn grandchildren_of_a_removed_child_are_flagged_deleted() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let mut parent = parent_with_children(&fx, &["a", "b"]).await?;
    let a = parent.child_by_label("a");
    let b = parent.child_by_label("b");
    for id in [a, b] {
        parent
            .children
            .get_persisted_mut(&id)
            .unwrap()
            .add_grandchild();
    }
    fx.parents.update(&mut parent).await?;

    let grandchild_of = |id: NcdChildId| {
        let pool = fx.pool.clone();
        async move {
            sqlx::query_scalar::<_, bool>(
                "SELECT deleted FROM ncd_grandchildren WHERE child_id = $1",
            )
            .bind(id)
            .fetch_one(&pool)
            .await
        }
    };
    assert!(!grandchild_of(a).await?);

    let mut parent = fx.parents.find_by_id(parent.id).await?;
    assert!(parent.remove_child(&a));
    fx.parents.update(&mut parent).await?;

    assert!(
        grandchild_of(a).await?,
        "removed child's grandchild is deleted"
    );
    assert!(
        !grandchild_of(b).await?,
        "sibling's grandchild is untouched"
    );
    Ok(())
}

#[tokio::test]
async fn delete_mutate_sibling_and_add_new_in_one_update() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let mut parent = parent_with_children(&fx, &["a", "b", "c"]).await?;
    let a = parent.child_by_label("a");
    let b = parent.child_by_label("b");

    assert!(parent.remove_child(&a));
    parent
        .children
        .get_persisted_mut(&b)
        .unwrap()
        .set_label("b2");
    parent.add_child("d");
    fx.parents.update(&mut parent).await?;

    let reloaded = fx.parents.find_by_id(parent.id).await?;
    let mut labels: Vec<_> = reloaded
        .children
        .iter_persisted()
        .map(|c| c.label.clone())
        .collect();
    labels.sort();
    assert_eq!(labels, vec!["b2", "c", "d"]);
    assert!(child_deleted(&fx.pool, a).await?);
    Ok(())
}

#[tokio::test]
async fn update_all_over_two_parents_each_deleting_a_child() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let mut p1 = parent_with_children(&fx, &["a", "b"]).await?;
    let mut p2 = parent_with_children(&fx, &["x", "y"]).await?;
    let a = p1.child_by_label("a");
    let x = p2.child_by_label("x");
    assert!(p1.remove_child(&a));
    assert!(p2.remove_child(&x));

    let mut batch = vec![p1, p2];
    fx.parents.update_all(&mut batch).await?;

    for (parent, gone) in batch.iter().zip([a, x]) {
        let reloaded = fx.parents.find_by_id(parent.id).await?;
        assert_eq!(reloaded.children.len_persisted(), 1);
        assert!(reloaded.children.get_persisted(&gone).is_none());
        assert!(child_deleted(&fx.pool, gone).await?);
    }
    Ok(())
}

#[tokio::test]
async fn post_persist_hook_sees_events_staged_on_a_deleted_child() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let mut parent = fx.parents.create(NewNcdParent::builder().build()).await?;
    parent
        .hooked
        .add_new(NewNcdHooked::builder().parent_id(parent.id).build());
    parent
        .hooked
        .add_new(NewNcdHooked::builder().parent_id(parent.id).build());
    fx.parents.update(&mut parent).await?;
    let mut parent = fx.parents.find_by_id(parent.id).await?;
    fx.hook_seen.lock().unwrap().clear();

    let ids: Vec<_> = parent.hooked.iter_persisted().map(|h| h.id).collect();
    parent.remove_hooked(&ids[0]);
    fx.parents.update(&mut parent).await?;

    // One hook call, for the removed child only, with its one staged event.
    assert_eq!(*fx.hook_seen.lock().unwrap(), vec![1]);
    let reloaded = fx.parents.find_by_id(parent.id).await?;
    assert_eq!(reloaded.hooked.len_persisted(), 1);
    Ok(())
}

#[tokio::test]
async fn removing_a_forgettable_child_erases_its_data() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let email = format!("secret-{}@example.com", NcdSecretId::new());
    let mut parent = fx.parents.create(NewNcdParent::builder().build()).await?;
    parent.secrets.add_new(
        NewNcdSecret::builder()
            .parent_id(parent.id)
            .name("Grace")
            .email(email.clone())
            .build(),
    );
    fx.parents.update(&mut parent).await?;
    let mut parent = fx.parents.find_by_id(parent.id).await?;
    let secret_id = parent.secrets.iter_persisted().next().unwrap().id;

    let payloads = |pool: PgPool| async move {
        sqlx::query_scalar::<_, i64>(
            "SELECT COUNT(*) FROM ncd_secrets_forgettable_payloads WHERE entity_id = $1",
        )
        .bind(secret_id)
        .fetch_one(&pool)
        .await
    };
    assert_eq!(payloads(fx.pool.clone()).await?, 1);

    parent.remove_secret(&secret_id);
    fx.parents.update(&mut parent).await?;

    let (deleted, stored): (bool, Option<String>) =
        sqlx::query_as("SELECT deleted, email FROM ncd_secrets WHERE id = $1")
            .bind(secret_id)
            .fetch_one(&fx.pool)
            .await?;
    assert!(deleted);
    assert_eq!(stored, None, "forgettable index column is NULLed");
    assert_eq!(
        payloads(fx.pool.clone()).await?,
        0,
        "payload rows are purged"
    );
    Ok(())
}

#[tokio::test]
async fn delete_all_in_op_on_a_flat_repo() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let children = &fx.parents.children;
    let parent = parent_with_children(&fx, &["a", "b", "c"]).await?;
    let ids = parent.child_ids();
    let mut loaded = children.find_all::<NcdChild>(&ids).await?;
    let mut entities: Vec<NcdChild> = ids.iter().map(|id| loaded.remove(id).unwrap()).collect();

    // An empty batch is a no-op.
    let mut op = fx.parents.begin_op().await?;
    children.delete_all_in_op(&mut op, Vec::new()).await?;
    op.commit().await?;

    // First entity has a staged event, second has a staged event and a
    // changed column, third has nothing staged.
    entities[0].retire();
    entities[1].set_label("changed");
    let staged_ids = [entities[0].id, entities[1].id];
    let plain_id = entities[2].id;

    let mut op = fx.parents.begin_op().await?;
    children.delete_all_in_op(&mut op, entities).await?;
    op.commit().await?;

    for id in ids {
        assert!(child_deleted(&fx.pool, id).await?);
    }
    assert_eq!(
        child_event_types(&fx.pool, staged_ids[0]).await?,
        vec!["initialized", "retired"]
    );
    assert_eq!(
        child_event_types(&fx.pool, staged_ids[1]).await?,
        vec!["initialized", "label_changed"]
    );
    assert_eq!(
        child_event_types(&fx.pool, plain_id).await?,
        vec!["initialized"]
    );
    let label: String = sqlx::query_scalar("SELECT label FROM ncd_children WHERE id = $1")
        .bind(staged_ids[1])
        .fetch_one(&fx.pool)
        .await?;
    assert_eq!(label, "changed");
    Ok(())
}

#[tokio::test]
async fn second_update_after_delete_issues_no_delete() -> anyhow::Result<()> {
    let fx = fixture().await?;
    let mut parent = parent_with_children(&fx, &["a", "b"]).await?;
    let a = parent.child_by_label("a");
    assert!(parent.remove_child(&a));
    fx.parents.update(&mut parent).await?;

    // The in-memory parent no longer lists the child, nor holds it as removed.
    assert!(parent.children.get_persisted(&a).is_none());
    assert_eq!(parent.children.len_removed(), 0);
    assert_eq!(parent.children.len_persisted(), 1);

    // A second update does not re-delete: the removed child's event count
    // is unchanged.
    fx.parents.update(&mut parent).await?;
    assert_eq!(
        child_event_types(&fx.pool, a).await?,
        vec!["initialized", "retired"]
    );
    // And removing an unknown child is a no-op.
    assert!(!parent.remove_child(&a));
    Ok(())
}

// ─── Statement counts ───────────────────────────────────────────────────
//
// Every generated bulk fn issues one combined statement per call, so counting
// calls to the child repo's `delete_all` (one span per call) counts the
// statements removals add. `NcdHookedRepo` has no grandchildren and nothing
// forgettable, so its `delete_all` is exactly one statement.
#[cfg(feature = "instrument")]
mod statement_counts {
    use std::collections::HashMap;

    use tracing_subscriber::layer::SubscriberExt;

    use super::*;

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

    async fn parent_with_hooked(fx: &Fixture, n: usize) -> anyhow::Result<NcdParent> {
        let mut parent = fx.parents.create(NewNcdParent::builder().build()).await?;
        for _ in 0..n {
            parent
                .hooked
                .add_new(NewNcdHooked::builder().parent_id(parent.id).build());
        }
        fx.parents.update(&mut parent).await?;
        Ok(fx.parents.find_by_id(parent.id).await?)
    }

    #[tokio::test]
    async fn removing_k_children_adds_one_delete_all_for_any_k() -> anyhow::Result<()> {
        let fx = fixture().await?;
        for k in [0usize, 1, 3] {
            let mut parent = parent_with_hooked(&fx, 4).await?;
            let ids: Vec<_> = parent.hooked.iter_persisted().map(|h| h.id).collect();
            for id in &ids[..k] {
                parent.remove_hooked(id);
            }

            let counts = SpanCounts::default();
            let subscriber = tracing_subscriber::registry().with(counts.clone());
            let _guard = tracing::subscriber::set_default(subscriber);
            fx.parents.update(&mut parent).await?;

            assert_eq!(
                counts.count("ncd_hooked_repo.delete_all"),
                usize::from(k > 0),
                "k = {k}"
            );
            // Nothing else about the child repo's writes changes.
            assert_eq!(counts.count("ncd_hooked_repo.update_all_mut"), 1, "k = {k}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn update_all_deleting_a_child_per_parent_is_one_delete_all() -> anyhow::Result<()> {
        let fx = fixture().await?;
        let mut batch = Vec::new();
        for _ in 0..3 {
            let mut parent = parent_with_hooked(&fx, 2).await?;
            let id = parent.hooked.iter_persisted().next().unwrap().id;
            parent.remove_hooked(&id);
            batch.push(parent);
        }

        let counts = SpanCounts::default();
        let subscriber = tracing_subscriber::registry().with(counts.clone());
        let _guard = tracing::subscriber::set_default(subscriber);
        fx.parents.update_all(&mut batch).await?;

        assert_eq!(counts.count("ncd_hooked_repo.delete_all"), 1);
        Ok(())
    }
}
