mod helpers;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use es_entity::*;

es_entity::entity_id! { GadgetId, KnobId, SprocketId, DoohickeyId }

// ---------------------------------------------------------------------------
// Gadget: default policy over a primary-key-only table
// ---------------------------------------------------------------------------

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "GadgetId")]
pub enum GadgetEvent {
    Initialized { id: GadgetId },
}

#[derive(EsEntity)]
pub struct Gadget {
    pub id: GadgetId,
    events: EntityEvents<GadgetEvent>,
}

impl TryFromEvents<GadgetEvent> for Gadget {
    fn try_from_events(events: EntityEvents<GadgetEvent>) -> Result<Self, EntityHydrationError> {
        let id = events
            .iter_all()
            .map(|GadgetEvent::Initialized { id }| *id)
            .next()
            .expect("an initialized event");
        Ok(Gadget { id, events })
    }
}

pub struct NewGadget;

impl IntoEvents<GadgetEvent> for NewGadget {
    type Id = Generated;

    fn into_events(
        self,
        builder: EntityEventsBuilder<GadgetEvent, Generated>,
    ) -> EntityEvents<GadgetEvent> {
        let id = builder.id();
        builder.initial_events([GadgetEvent::Initialized { id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Gadget")]
pub struct Gadgets {
    pool: PgPool,
}

impl Gadgets {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// ---------------------------------------------------------------------------
// Knob: default policy, a unique column stays in the family
// ---------------------------------------------------------------------------

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "KnobId")]
pub enum KnobEvent {
    Initialized { id: KnobId, serial: String },
}

#[derive(EsEntity)]
pub struct Knob {
    pub id: KnobId,
    pub serial: String,
    events: EntityEvents<KnobEvent>,
}

impl TryFromEvents<KnobEvent> for Knob {
    fn try_from_events(events: EntityEvents<KnobEvent>) -> Result<Self, EntityHydrationError> {
        let (id, serial) = events
            .iter_all()
            .map(|KnobEvent::Initialized { id, serial }| (*id, serial.clone()))
            .next()
            .expect("an initialized event");
        Ok(Knob { id, serial, events })
    }
}

pub struct NewKnob {
    pub serial: String,
}

impl IntoEvents<KnobEvent> for NewKnob {
    type Id = Generated;

    fn into_events(
        self,
        builder: EntityEventsBuilder<KnobEvent, Generated>,
    ) -> EntityEvents<KnobEvent> {
        let id = builder.id();
        builder.initial_events([KnobEvent::Initialized {
            id,
            serial: self.serial,
        }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Knob", columns(serial(ty = "String")))]
pub struct Knobs {
    pool: PgPool,
}

impl Knobs {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// ---------------------------------------------------------------------------
// Sprocket: a custom generator
// ---------------------------------------------------------------------------

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "SprocketId")]
pub enum SprocketEvent {
    Initialized { id: SprocketId },
}

#[derive(EsEntity)]
pub struct Sprocket {
    pub id: SprocketId,
    events: EntityEvents<SprocketEvent>,
}

impl TryFromEvents<SprocketEvent> for Sprocket {
    fn try_from_events(events: EntityEvents<SprocketEvent>) -> Result<Self, EntityHydrationError> {
        let id = events
            .iter_all()
            .map(|SprocketEvent::Initialized { id }| *id)
            .next()
            .expect("an initialized event");
        Ok(Sprocket { id, events })
    }
}

pub struct NewSprocket;

impl IntoEvents<SprocketEvent> for NewSprocket {
    type Id = Generated;

    fn into_events(
        self,
        builder: EntityEventsBuilder<SprocketEvent, Generated>,
    ) -> EntityEvents<SprocketEvent> {
        let id = builder.id();
        builder.initial_events([SprocketEvent::Initialized { id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Sprocket", id(generated(by = "Self::next_id")))]
pub struct Sprockets {
    pool: PgPool,
    minted: std::sync::Arc<std::sync::Mutex<Vec<SprocketId>>>,
}

impl Sprockets {
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            minted: Default::default(),
        }
    }

    async fn next_id<OP>(&self, _op: &mut OP) -> Result<SprocketId, es_entity::RepoFault>
    where
        OP: es_entity::AtomicOperation + ?Sized,
    {
        let id = SprocketId::new();
        self.minted.lock().unwrap().push(id);
        Ok(id)
    }
}

// ---------------------------------------------------------------------------
// Doohickey: a supplied id whose `New*` hands the container a different id
// ---------------------------------------------------------------------------

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "DoohickeyId")]
pub enum DoohickeyEvent {
    Initialized { id: DoohickeyId },
}

#[derive(EsEntity)]
pub struct Doohickey {
    pub id: DoohickeyId,
    events: EntityEvents<DoohickeyEvent>,
}

impl TryFromEvents<DoohickeyEvent> for Doohickey {
    fn try_from_events(events: EntityEvents<DoohickeyEvent>) -> Result<Self, EntityHydrationError> {
        let id = events
            .iter_all()
            .map(|DoohickeyEvent::Initialized { id }| *id)
            .next()
            .expect("an initialized event");
        Ok(Doohickey { id, events })
    }
}

pub struct NewDoohickey {
    pub id: DoohickeyId,
    pub container_id: DoohickeyId,
}

impl IntoEvents<DoohickeyEvent> for NewDoohickey {
    type Id = Supplied;

    fn into_events(
        self,
        builder: EntityEventsBuilder<DoohickeyEvent, Supplied>,
    ) -> EntityEvents<DoohickeyEvent> {
        builder.initial_events(
            self.container_id,
            [DoohickeyEvent::Initialized { id: self.id }],
        )
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Doohickey", id(supplied))]
pub struct Doohickeys {
    pool: PgPool,
}

impl Doohickeys {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

// ---------------------------------------------------------------------------

fn takes_fault<T>(res: Result<T, es_entity::RepoFault>) -> Result<T, es_entity::RepoFault> {
    res
}

#[tokio::test]
async fn pk_only_table_on_the_default_policy_creates_with_repo_fault() -> anyhow::Result<()> {
    let gadgets = Gadgets::new(helpers::init_pool().await?);

    let gadget = takes_fault(gadgets.create(NewGadget).await)?;

    let reloaded = gadgets.find_by_id(gadget.id).await?;
    assert_eq!(reloaded.id, gadget.id);
    assert_eq!(*gadget.events().id(), gadget.id);
    Ok(())
}

#[tokio::test]
async fn create_all_resolves_a_distinct_id_per_entity() -> anyhow::Result<()> {
    let gadgets = Gadgets::new(helpers::init_pool().await?);

    let created = takes_fault(
        gadgets
            .create_all(vec![NewGadget, NewGadget, NewGadget])
            .await,
    )?;

    let mut ids: Vec<_> = created.iter().map(|g| g.id).collect();
    ids.sort_by_key(|id| id.to_string());
    ids.dedup();
    assert_eq!(ids.len(), 3);
    for gadget in &created {
        assert_eq!(gadgets.find_by_id(gadget.id).await?.id, gadget.id);
    }
    Ok(())
}

#[tokio::test]
async fn unique_column_stays_in_the_family_without_a_pkey_case() -> anyhow::Result<()> {
    let knobs = Knobs::new(helpers::init_pool().await?);
    let serial = format!("serial-{}", KnobId::new());

    knobs
        .create(NewKnob {
            serial: serial.clone(),
        })
        .await?;
    let err = knobs
        .create(NewKnob { serial })
        .await
        .err()
        .expect("a duplicate serial must be rejected");

    let Fail::Rejected(violation) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    // Exhaustive on purpose: the only case is the unique column, there is no
    // `Pkey` to name.
    match violation {
        KnobConstraintViolation::SerialKey(conflict) => {
            assert!(conflict.attempted.is_some());
        }
    }
    Ok(())
}

#[tokio::test]
async fn custom_generator_supplies_the_id() -> anyhow::Result<()> {
    let sprockets = Sprockets::new(helpers::init_pool().await?);

    let sprocket = sprockets.create(NewSprocket).await?;

    assert_eq!(sprockets.minted.lock().unwrap().as_slice(), [sprocket.id]);
    Ok(())
}

#[tokio::test]
async fn supplied_id_that_differs_from_the_container_id_is_a_fault() -> anyhow::Result<()> {
    let doohickeys = Doohickeys::new(helpers::init_pool().await?);

    let err = doohickeys
        .create(NewDoohickey {
            id: DoohickeyId::new(),
            container_id: DoohickeyId::new(),
        })
        .await
        .err()
        .expect("an entity built under another id must not be inserted");

    assert!(matches!(err, Fail::Fatal(_)), "got {err:?}");
    Ok(())
}

#[tokio::test]
async fn create_all_with_a_supplied_id_that_differs_from_the_container_id_writes_nothing()
-> anyhow::Result<()> {
    let doohickeys = Doohickeys::new(helpers::init_pool().await?);
    let consistent = DoohickeyId::new();
    let mismatched = DoohickeyId::new();

    let mut op = doohickeys.begin_op().await?;
    let err = doohickeys
        .create_all_in_op(
            &mut op,
            vec![
                NewDoohickey {
                    id: consistent,
                    container_id: consistent,
                },
                NewDoohickey {
                    id: mismatched,
                    container_id: DoohickeyId::new(),
                },
            ],
        )
        .await
        .err()
        .expect("a batch with an entity built under another id must not be inserted");
    assert!(matches!(err, Fail::Fatal(_)), "got {err:?}");

    assert!(
        doohickeys
            .maybe_find_by_id_in_op(&mut op, consistent)
            .await?
            .is_none()
    );
    assert!(
        doohickeys
            .maybe_find_by_id_in_op(&mut op, mismatched)
            .await?
            .is_none()
    );
    Ok(())
}

#[tokio::test]
async fn hydrating_an_entity_whose_payload_id_differs_from_the_container_is_corrupt_state()
-> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let gadgets = Gadgets::new(pool.clone());
    let stored_under = GadgetId::new();
    let claims_to_be = GadgetId::new();

    sqlx::query("INSERT INTO gadgets (id, created_at) VALUES ($1, NOW())")
        .bind(uuid::Uuid::from(stored_under))
        .execute(&pool)
        .await?;
    sqlx::query(
        "INSERT INTO gadget_events (id, sequence, event_type, event, recorded_at) \
         VALUES ($1, 1, 'initialized', $2, NOW())",
    )
    .bind(uuid::Uuid::from(stored_under))
    .bind(serde_json::json!({ "type": "initialized", "id": claims_to_be }))
    .execute(&pool)
    .await?;

    let err = gadgets
        .find_by_id(stored_under)
        .await
        .err()
        .expect("a mismatched payload id is corrupt state");

    match err {
        Fault::Fatal(fatal) => assert_eq!(fatal.kind, FatalKind::CorruptState),
        other => panic!("expected Fatal(CorruptState), got {other:?}"),
    }
    Ok(())
}
