mod helpers;

use serde::{Deserialize, Serialize};
use sqlx::PgPool;

use es_entity::*;

es_entity::entity_id! { DoohickeyId, KnobId }

// ---------------------------------------------------------------------------
// Doohickey: a deterministic id over a primary-key-only table
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
        Ok(Doohickey {
            id: *events.id(),
            events,
        })
    }
}

pub struct NewDoohickey {
    pub id: DoohickeyId,
}

impl IntoEvents<DoohickeyEvent> for NewDoohickey {
    type Id = Supplied;

    fn into_events(
        self,
        builder: EntityEventsBuilder<DoohickeyEvent, Supplied>,
    ) -> EntityEvents<DoohickeyEvent> {
        builder.initial_events(self.id, [DoohickeyEvent::Initialized { id: self.id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Doohickey", id = supplied)]
pub struct Doohickeys {
    pool: PgPool,
}

// ---------------------------------------------------------------------------
// Knob: a deterministic id next to a unique column
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
    pub id: KnobId,
    pub serial: String,
}

impl IntoEvents<KnobEvent> for NewKnob {
    type Id = Supplied;

    fn into_events(
        self,
        builder: EntityEventsBuilder<KnobEvent, Supplied>,
    ) -> EntityEvents<KnobEvent> {
        builder.initial_events(
            self.id,
            [KnobEvent::Initialized {
                id: self.id,
                serial: self.serial,
            }],
        )
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Knob", id = supplied, columns(serial(ty = "String")))]
pub struct Knobs {
    pool: PgPool,
}

// ---------------------------------------------------------------------------

fn takes_fault<T>(res: Result<T, es_entity::RepoFault>) -> Result<T, es_entity::RepoFault> {
    res
}

#[tokio::test]
async fn second_call_for_the_same_id_finds_the_stored_entity() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = Doohickeys { pool: pool.clone() };
    let id = DoohickeyId::new();

    let mut op = DbOp::init(&pool).await?;
    let first = takes_fault(
        repo.create_or_find_by_id_in_op(&mut op, NewDoohickey { id })
            .await,
    )?;
    let second = takes_fault(
        repo.create_or_find_by_id_in_op(&mut op, NewDoohickey { id })
            .await,
    )?;
    // The conflict ran on a savepoint: the operation is still usable.
    let other = DoohickeyId::new();
    let third = takes_fault(
        repo.create_or_find_by_id_in_op(&mut op, NewDoohickey { id: other })
            .await,
    )?;
    op.commit().await?;

    assert!(matches!(&first, Ensured::Created(d) if d.id == id));
    assert!(matches!(&second, Ensured::Found(d) if d.id == id));
    assert!(third.is_created());
    assert_eq!(repo.find_by_id(id).await?.id, id);
    Ok(())
}

#[tokio::test]
async fn concurrent_calls_for_one_id_create_exactly_once() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let id = DoohickeyId::new();

    let mut tasks = Vec::new();
    for _ in 0..8 {
        let pool = pool.clone();
        tasks.push(tokio::spawn(async move {
            let repo = Doohickeys { pool: pool.clone() };
            let mut op = DbOp::init(&pool).await?;
            let ensured = repo
                .create_or_find_by_id_in_op(&mut op, NewDoohickey { id })
                .await
                .map_err(|e| anyhow::anyhow!("{e:?}"))?;
            op.commit().await?;
            anyhow::Ok(ensured)
        }));
    }
    let mut created = 0;
    let mut found = 0;
    for task in tasks {
        match task.await?? {
            Ensured::Created(d) => {
                assert_eq!(d.id, id);
                created += 1;
            }
            Ensured::Found(d) => {
                assert_eq!(d.id, id);
                found += 1;
            }
        }
    }
    assert_eq!((created, found), (1, 7));
    Ok(())
}

#[tokio::test]
async fn a_conflict_on_another_constraint_still_rejects() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = Knobs { pool: pool.clone() };
    let serial = format!("serial-{}", KnobId::new());

    let mut op = DbOp::init(&pool).await?;
    repo.create_or_find_by_id_in_op(
        &mut op,
        NewKnob {
            id: KnobId::new(),
            serial: serial.clone(),
        },
    )
    .await?;
    op.commit().await?;

    let mut op = DbOp::init(&pool).await?;
    let err = repo
        .create_or_find_by_id_in_op(
            &mut op,
            NewKnob {
                id: KnobId::new(),
                serial,
            },
        )
        .await
        .err()
        .expect("a different id with the same serial must be rejected");
    let Fail::Rejected(violation) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    assert!(matches!(violation, KnobConstraintViolation::SerialKey(_)));
    Ok(())
}
