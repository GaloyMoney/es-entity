mod helpers;

use std::{
    collections::BTreeSet,
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use es_entity::errlanes::{Fail, RejectionCode};
use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! {
    EnsureTemplateId,
    EnsureSlotId,
    EnsureAccountId,
    EnsurePartialId,
    DoohickeyId,
    KnobId
}

macro_rules! ensure_entity {
    ($entity:ident, $new:ident, $event:ident, $id:ident, $id_name:literal) => {
        #[derive(EsEvent, Debug, Serialize, Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case")]
        #[es_event(id = $id_name)]
        pub enum $event {
            Initialized {
                id: $id,
                code: String,
                account: String,
                currency: String,
                label: String,
                note: String,
            },
        }

        #[derive(EsEntity)]
        pub struct $entity {
            pub id: $id,
            pub code: String,
            pub account: String,
            pub currency: String,
            pub label: String,
            pub note: String,
            events: EntityEvents<$event>,
        }

        impl TryFromEvents<$event> for $entity {
            fn try_from_events(events: EntityEvents<$event>) -> Result<Self, EntityHydrationError> {
                let (id, code, account, currency, label, note) = events
                    .iter_all()
                    .map(
                        |$event::Initialized {
                             id,
                             code,
                             account,
                             currency,
                             label,
                             note,
                         }| {
                            (
                                *id,
                                code.clone(),
                                account.clone(),
                                currency.clone(),
                                label.clone(),
                                note.clone(),
                            )
                        },
                    )
                    .next()
                    .expect("an initialized event");
                Ok($entity {
                    id,
                    code,
                    account,
                    currency,
                    label,
                    note,
                    events,
                })
            }
        }

        #[derive(Clone)]
        pub struct $new {
            pub code: String,
            pub account: String,
            pub currency: String,
            pub label: String,
            pub note: String,
        }

        impl IntoEvents<$event> for $new {
            type IdSource = FromRepo;

            fn into_events(
                self,
                builder: EntityEventsBuilder<$event, FromRepo>,
            ) -> EntityEvents<$event> {
                let id = builder.id();
                builder.initial_events([$event::Initialized {
                    id,
                    code: self.code,
                    account: self.account,
                    currency: self.currency,
                    label: self.label,
                    note: self.note,
                }])
            }
        }
    };
}

ensure_entity!(
    EnsureTemplate,
    NewEnsureTemplate,
    EnsureTemplateEvent,
    EnsureTemplateId,
    "EnsureTemplateId"
);
ensure_entity!(
    EnsureSlot,
    NewEnsureSlot,
    EnsureSlotEvent,
    EnsureSlotId,
    "EnsureSlotId"
);

static HOOK_RUNS: AtomicUsize = AtomicUsize::new(0);

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "EnsureTemplate",
    columns(code(ty = "String")),
    post_persist_hook = "count_persist"
)]
pub struct EnsureTemplates {
    pool: PgPool,
}

impl EnsureTemplates {
    async fn count_persist<OP: es_entity::AtomicOperation>(
        &self,
        _op: &mut OP,
        _entity: &EnsureTemplate,
        _new_events: es_entity::events::LastPersisted<'_, EnsureTemplateEvent>,
    ) -> Result<(), RepoFault> {
        HOOK_RUNS.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "EnsureSlot",
    columns(account(ty = "String"), currency(ty = "String"), label(ty = "String"))
)]
pub struct EnsureSlots {
    pool: PgPool,
}

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "EnsureAccountId")]
pub enum EnsureAccountEvent {
    Initialized {
        id: EnsureAccountId,
        code: String,
        external_id: Option<String>,
        note: String,
    },
}

#[derive(EsEntity)]
pub struct EnsureAccount {
    pub id: EnsureAccountId,
    pub code: String,
    pub external_id: Option<String>,
    pub note: String,
    events: EntityEvents<EnsureAccountEvent>,
}

impl TryFromEvents<EnsureAccountEvent> for EnsureAccount {
    fn try_from_events(
        events: EntityEvents<EnsureAccountEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let (id, code, external_id, note) = events
            .iter_all()
            .map(
                |EnsureAccountEvent::Initialized {
                     id,
                     code,
                     external_id,
                     note,
                 }| { (*id, code.clone(), external_id.clone(), note.clone()) },
            )
            .next()
            .expect("an initialized event");
        Ok(EnsureAccount {
            id,
            code,
            external_id,
            note,
            events,
        })
    }
}

pub struct NewEnsureAccount {
    pub id: EnsureAccountId,
    pub code: String,
    pub external_id: Option<String>,
    pub note: String,
}

impl IntoEvents<EnsureAccountEvent> for NewEnsureAccount {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<EnsureAccountEvent, FromCaller>,
    ) -> EntityEvents<EnsureAccountEvent> {
        builder.initial_events(
            self.id,
            [EnsureAccountEvent::Initialized {
                id: self.id,
                code: self.code,
                external_id: self.external_id,
                note: self.note,
            }],
        )
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "EnsureAccount",
    id(source = caller),
    columns(code(ty = "String"), external_id(ty = "Option<String>"))
)]
pub struct EnsureAccounts {
    pool: PgPool,
}

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "EnsurePartialId")]
pub enum EnsurePartialEvent {
    Initialized { id: EnsurePartialId, tag: String },
}

#[derive(EsEntity)]
pub struct EnsurePartial {
    pub id: EnsurePartialId,
    pub tag: String,
    events: EntityEvents<EnsurePartialEvent>,
}

impl TryFromEvents<EnsurePartialEvent> for EnsurePartial {
    fn try_from_events(
        events: EntityEvents<EnsurePartialEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let (id, tag) = events
            .iter_all()
            .map(|EnsurePartialEvent::Initialized { id, tag }| (*id, tag.clone()))
            .next()
            .expect("an initialized event");
        Ok(EnsurePartial { id, tag, events })
    }
}

pub struct NewEnsurePartial {
    pub id: EnsurePartialId,
    pub tag: String,
}

impl IntoEvents<EnsurePartialEvent> for NewEnsurePartial {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<EnsurePartialEvent, FromCaller>,
    ) -> EntityEvents<EnsurePartialEvent> {
        builder.initial_events(
            self.id,
            [EnsurePartialEvent::Initialized {
                id: self.id,
                tag: self.tag,
            }],
        )
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "EnsurePartial",
    id(source = caller),
    columns(tag(ty = "String"))
)]
pub struct EnsurePartials {
    pool: PgPool,
}

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
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<DoohickeyEvent, FromCaller>,
    ) -> EntityEvents<DoohickeyEvent> {
        builder.initial_events(self.id, [DoohickeyEvent::Initialized { id: self.id }])
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "Doohickey", id(source = caller))]
pub struct Doohickeys {
    pool: PgPool,
}

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
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<KnobEvent, FromCaller>,
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
#[es_repo(entity = "Knob", id(source = caller), columns(serial(ty = "String")))]
pub struct Knobs {
    pool: PgPool,
}

fn template(code: &str, note: &str) -> NewEnsureTemplate {
    NewEnsureTemplate {
        code: code.to_string(),
        account: String::new(),
        currency: String::new(),
        label: String::new(),
        note: note.to_string(),
    }
}

fn slot(account: &str, currency: &str, label: &str) -> NewEnsureSlot {
    NewEnsureSlot {
        code: String::new(),
        account: account.to_string(),
        currency: currency.to_string(),
        label: label.to_string(),
        note: String::new(),
    }
}

fn account(
    id: EnsureAccountId,
    code: &str,
    external_id: Option<&str>,
    note: &str,
) -> NewEnsureAccount {
    NewEnsureAccount {
        id,
        code: code.to_string(),
        external_id: external_id.map(str::to_string),
        note: note.to_string(),
    }
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", EnsureTemplateId::new())
}

fn takes_fault<T>(res: Result<T, RepoFault>) -> Result<T, RepoFault> {
    res
}

async fn event_count(pool: &PgPool, code: &str) -> anyhow::Result<i64> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM ensure_template_events e \
         JOIN ensure_templates t ON t.id = e.id WHERE t.code = $1",
    )
    .bind(code)
    .fetch_one(pool)
    .await?)
}

#[tokio::test]
async fn creates_then_finds() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureTemplates { pool: pool.clone() };
    let code = unique("seq");

    let mut op = DbOp::init(&pool).await?;
    let first = repo
        .create_or_find_in_op(&mut op, template(&code, "first"))
        .await?;
    let second = repo
        .create_or_find_in_op(&mut op, template(&code, "second"))
        .await?;
    let other = repo
        .create_or_find_in_op(&mut op, template(&unique("other"), "third"))
        .await?;
    op.commit().await?;

    let Ensured::Created(created) = first else {
        panic!("first call must create")
    };
    let Ensured::Found(found) = second else {
        panic!("second call must find")
    };
    assert_eq!(created.id, found.id);
    assert!(other.is_created());
    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ensure_templates WHERE code = $1")
        .bind(&code)
        .fetch_one(&pool)
        .await?;
    assert_eq!(rows, 1);
    assert_eq!(event_count(&pool, &code).await?, 1);
    Ok(())
}

#[tokio::test]
async fn found_does_not_persist_new_events_or_run_hook() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureTemplates { pool: pool.clone() };
    let code = unique("hook");

    let mut op = DbOp::init(&pool).await?;
    let before_create = HOOK_RUNS.load(Ordering::SeqCst);
    let first = repo
        .create_or_find_in_op(&mut op, template(&code, "kept"))
        .await?;
    let after_create = HOOK_RUNS.load(Ordering::SeqCst);
    let second = repo
        .create_or_find_in_op(&mut op, template(&code, "dropped"))
        .await?;
    let after_find = HOOK_RUNS.load(Ordering::SeqCst);
    op.commit().await?;

    assert!(first.is_created());
    let Ensured::Found(found) = second else {
        panic!("second call must find")
    };
    assert_eq!(found.note, "kept");
    assert!(after_create > before_create);
    assert_eq!(after_find, after_create);
    assert_eq!(event_count(&pool, &code).await?, 1);
    Ok(())
}

#[tokio::test]
async fn concurrent_ops_one_created_one_found() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let code = unique("concurrent");

    let repo = EnsureTemplates { pool: pool.clone() };
    let mut first_op = DbOp::init(&pool).await?;
    let first = repo
        .create_or_find_in_op(&mut first_op, template(&code, "winner"))
        .await?;

    let second_pool = pool.clone();
    let second_code = code.clone();
    let second = tokio::spawn(async move {
        let repo = EnsureTemplates {
            pool: second_pool.clone(),
        };
        let mut op = DbOp::init(&second_pool).await?;
        let ensured = repo
            .create_or_find_in_op(&mut op, template(&second_code, "loser"))
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        op.commit().await?;
        anyhow::Ok(ensured)
    });

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !second.is_finished(),
        "the second op must block on the uncommitted key"
    );
    first_op.commit().await?;

    let second = second.await??;
    let Ensured::Created(created) = first else {
        panic!("the first op creates")
    };
    let Ensured::Found(found) = second else {
        panic!("the blocked op finds")
    };
    assert_eq!(created.id, found.id);
    assert_eq!(found.note, "winner");
    assert_eq!(event_count(&pool, &code).await?, 1);
    Ok(())
}

#[tokio::test]
async fn rolled_back_holder_lets_blocked_op_create() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let code = unique("rollback");

    let repo = EnsureTemplates { pool: pool.clone() };
    let mut first_op = DbOp::init(&pool).await?;
    repo.create_or_find_in_op(&mut first_op, template(&code, "abandoned"))
        .await?;

    let second_pool = pool.clone();
    let second_code = code.clone();
    let second = tokio::spawn(async move {
        let repo = EnsureTemplates {
            pool: second_pool.clone(),
        };
        let mut op = DbOp::init(&second_pool).await?;
        let ensured = repo
            .create_or_find_in_op(&mut op, template(&second_code, "kept"))
            .await
            .map_err(|e| anyhow::anyhow!("{e:?}"))?;
        op.commit().await?;
        anyhow::Ok(ensured)
    });
    tokio::time::sleep(Duration::from_millis(300)).await;
    drop(first_op);

    let second = second.await??;
    let Ensured::Created(created) = second else {
        panic!("the blocked op creates once the holder rolled back")
    };
    assert_eq!(created.note, "kept");
    Ok(())
}

#[tokio::test]
async fn eight_way_concurrency_creates_once() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let code = unique("many");
    let mut tasks = Vec::new();
    for i in 0..8 {
        let pool = pool.clone();
        let code = code.clone();
        tasks.push(tokio::spawn(async move {
            let repo = EnsureTemplates { pool: pool.clone() };
            let mut op = DbOp::init(&pool).await?;
            let ensured = repo
                .create_or_find_in_op(&mut op, template(&code, &format!("n{i}")))
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
            Ensured::Created(_) => created += 1,
            Ensured::Found(_) => found += 1,
        }
    }
    assert_eq!((created, found), (1, 7));
    assert_eq!(event_count(&pool, &code).await?, 1);
    Ok(())
}

#[tokio::test]
async fn single_key_repo_returns_repo_fault() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = Doohickeys { pool: pool.clone() };
    let id = DoohickeyId::new();

    let mut op = DbOp::init(&pool).await?;
    let first = takes_fault(
        repo.create_or_find_in_op(&mut op, NewDoohickey { id })
            .await,
    )?;
    let second = takes_fault(
        repo.create_or_find_in_op(&mut op, NewDoohickey { id })
            .await,
    )?;
    let other = DoohickeyId::new();
    let third = takes_fault(
        repo.create_or_find_in_op(&mut op, NewDoohickey { id: other })
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
                .create_or_find_in_op(&mut op, NewDoohickey { id })
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
async fn found_through_composite_key() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSlots { pool: pool.clone() };
    let account = unique("account");

    let mut op = DbOp::init(&pool).await?;
    let first = repo
        .create_or_find_in_op(&mut op, slot(&account, "USD", &unique("label")))
        .await?;
    let second = repo
        .create_or_find_in_op(&mut op, slot(&account, "USD", &unique("label")))
        .await?;
    let other_currency = repo
        .create_or_find_in_op(&mut op, slot(&account, "EUR", &unique("label")))
        .await?;
    op.commit().await?;

    let Ensured::Created(created) = first else {
        panic!("first call must create")
    };
    let Ensured::Found(found) = second else {
        panic!("same account and currency must find")
    };
    assert_eq!(created.id, found.id);
    assert!(other_currency.is_created());
    Ok(())
}

#[tokio::test]
async fn found_through_non_id_key_returns_stored_id() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureAccounts { pool: pool.clone() };
    let stored_id = EnsureAccountId::new();
    let fresh_id = EnsureAccountId::new();
    let code = unique("code");

    let mut op = DbOp::init(&pool).await?;
    let first = repo
        .create_or_find_in_op(&mut op, account(stored_id, &code, None, "stored"))
        .await?;
    let second = repo
        .create_or_find_in_op(&mut op, account(fresh_id, &code, None, "ignored"))
        .await?;
    op.commit().await?;

    assert!(matches!(&first, Ensured::Created(a) if a.id == stored_id));
    let Ensured::Found(found) = second else {
        panic!("the same code under another id must find")
    };
    assert_eq!(found.id, stored_id);
    assert_eq!(found.note, "stored");
    let fresh_rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ensure_accounts WHERE id = $1")
        .bind(uuid::Uuid::from(fresh_id))
        .fetch_one(&pool)
        .await?;
    assert_eq!(fresh_rows, 0);
    Ok(())
}

#[tokio::test]
async fn found_through_a_taken_id_ignores_a_new_code() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = Knobs { pool: pool.clone() };
    let id = KnobId::new();
    let serial = format!("serial-{}", KnobId::new());

    let mut op = DbOp::init(&pool).await?;
    let first = repo
        .create_or_find_in_op(
            &mut op,
            NewKnob {
                id,
                serial: serial.clone(),
            },
        )
        .await?;
    let second = repo
        .create_or_find_in_op(
            &mut op,
            NewKnob {
                id,
                serial: format!("serial-{}", KnobId::new()),
            },
        )
        .await?;
    let third = repo
        .create_or_find_in_op(
            &mut op,
            NewKnob {
                id: KnobId::new(),
                serial: serial.clone(),
            },
        )
        .await?;
    op.commit().await?;

    assert!(first.is_created());
    let Ensured::Found(by_id) = second else {
        panic!("a taken id must find")
    };
    let Ensured::Found(by_serial) = third else {
        panic!("a taken serial must find")
    };
    assert_eq!((by_id.id, by_serial.id), (id, id));
    assert_eq!(by_id.serial, serial);
    Ok(())
}

#[tokio::test]
async fn absent_optional_key_does_not_match() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureAccounts { pool: pool.clone() };

    let mut op = DbOp::init(&pool).await?;
    let stored = repo
        .create_or_find_in_op(
            &mut op,
            account(EnsureAccountId::new(), &unique("code"), None, "stored"),
        )
        .await?;
    let absent = repo
        .create_or_find_in_op(
            &mut op,
            account(EnsureAccountId::new(), &unique("code"), None, "also absent"),
        )
        .await?;
    op.commit().await?;

    assert!(stored.is_created());
    assert!(absent.is_created());
    Ok(())
}

#[tokio::test]
async fn keys_span_entities_is_rejected() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureAccounts { pool: pool.clone() };
    let x = EnsureAccountId::new();
    let y = EnsureAccountId::new();
    let code = unique("code");
    let external_id = unique("external");

    let mut op = DbOp::init(&pool).await?;
    repo.create_in_op(&mut op, account(x, &code, None, "x"))
        .await?;
    repo.create_in_op(
        &mut op,
        account(y, &unique("code"), Some(&external_id), "y"),
    )
    .await?;

    let err = repo
        .create_or_find_in_op(
            &mut op,
            account(
                EnsureAccountId::new(),
                &code,
                Some(&external_id),
                "straddles",
            ),
        )
        .await
        .err()
        .expect("keys held by two entities must reject");
    let Fail::Rejected(rejection) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    let EnsureAccountEnsureRejection::KeysSpanEntities { ids, keys } = rejection;
    assert_eq!(
        ids.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([x, y])
    );
    assert_eq!(
        keys.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from(["code", "external_id"])
    );

    let after = repo
        .create_in_op(
            &mut op,
            account(EnsureAccountId::new(), &unique("code"), None, "after"),
        )
        .await?;
    op.commit().await?;
    assert_eq!(after.note, "after");
    Ok(())
}

#[tokio::test]
async fn keys_span_entities_includes_id() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureAccounts { pool: pool.clone() };
    let x = EnsureAccountId::new();
    let y = EnsureAccountId::new();
    let code = unique("code");

    let mut op = DbOp::init(&pool).await?;
    repo.create_in_op(&mut op, account(x, &unique("code"), None, "x"))
        .await?;
    repo.create_in_op(&mut op, account(y, &code, None, "y"))
        .await?;

    let err = repo
        .create_or_find_in_op(&mut op, account(x, &code, None, "straddles"))
        .await
        .err()
        .expect("an id and a code held by two entities must reject");
    let Fail::Rejected(rejection) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    let EnsureAccountEnsureRejection::KeysSpanEntities { ids, keys } = rejection;
    assert_eq!(
        ids.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([x, y])
    );
    assert_eq!(
        keys.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from(["id", "code"])
    );
    Ok(())
}

#[tokio::test]
async fn keys_span_entities_carries_ids_and_names_not_values() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSlots { pool: pool.clone() };
    let secret = unique("secret-label");
    let account_a = unique("account");
    let account_b = unique("account");

    let mut op = DbOp::init(&pool).await?;
    repo.create_in_op(&mut op, slot(&account_a, "USD", &unique("label")))
        .await?;
    repo.create_in_op(&mut op, slot(&account_b, "EUR", &secret))
        .await?;
    let err = repo
        .create_or_find_in_op(&mut op, slot(&account_a, "USD", &secret))
        .await
        .err()
        .expect("a composite key and a label held by two entities must reject");
    let Fail::Rejected(rejection) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    let EnsureSlotEnsureRejection::KeysSpanEntities { keys, .. } = &rejection;
    assert_eq!(
        keys.iter().copied().collect::<BTreeSet<_>>(),
        BTreeSet::from(["account_and_currency", "label"])
    );
    for rendered in [format!("{rejection}"), format!("{rejection:?}")] {
        assert!(!rendered.contains(&secret), "{rendered}");
        assert!(!rendered.contains(&account_a), "{rendered}");
    }
    assert_eq!(EnsureSlotEnsureRejectionCode::CODES.len(), 1);
    assert_eq!(
        EnsureSlotEnsureRejectionCode::CODES[0].code,
        "ensure_slots_keys_span_entities"
    );
    Ok(())
}

#[tokio::test]
async fn partial_unique_index_conflict_is_transient() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsurePartials { pool: pool.clone() };
    let tag = unique("tag");

    let mut op = DbOp::init(&pool).await?;
    repo.create_in_op(
        &mut op,
        NewEnsurePartial {
            id: EnsurePartialId::new(),
            tag: tag.clone(),
        },
    )
    .await?;

    let arbitrated = sqlx::query(
        "INSERT INTO ensure_partials (id, tag, created_at) VALUES ($1, $2, NOW()) ON CONFLICT DO NOTHING",
    )
    .bind(uuid::Uuid::from(EnsurePartialId::new()))
    .bind(&tag)
    .execute(op.as_executor())
    .await?;
    assert_eq!(
        arbitrated.rows_affected(),
        0,
        "the partial index must arbitrate the conflict"
    );

    let fault = takes_fault(
        repo.create_or_find_in_op(
            &mut op,
            NewEnsurePartial {
                id: EnsurePartialId::new(),
                tag: tag.clone(),
            },
        )
        .await,
    )
    .err()
    .expect("a conflict no key explains cannot be found");
    assert!(fault.is_transient(), "{fault:?}");
    assert!(!fault.is_fatal(), "{fault:?}");
    Ok(())
}

async fn repeatable_read_after_concurrent_commit(
    level: &str,
) -> anyhow::Result<
    Result<Ensured<EnsureTemplate>, es_entity::RepoWriteError<EnsureTemplateEnsureRejection>>,
> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureTemplates { pool: pool.clone() };
    let code = unique("isolation");

    let mut tx = pool.begin().await?;
    sqlx::query(&format!("SET TRANSACTION ISOLATION LEVEL {level}"))
        .execute(&mut *tx)
        .await?;
    sqlx::query("SELECT 1").execute(&mut *tx).await?;
    sqlx::query("SELECT COUNT(*) FROM ensure_templates")
        .execute(&mut *tx)
        .await?;

    let mut other = DbOp::init(&pool).await?;
    repo.create_or_find_in_op(&mut other, template(&code, "committed elsewhere"))
        .await?;
    other.commit().await?;

    Ok(repo
        .create_or_find_in_op(&mut tx, template(&code, "late"))
        .await)
}

#[tokio::test]
async fn under_snapshot_isolation_is_transient_never_corrupt() -> anyhow::Result<()> {
    for level in ["REPEATABLE READ", "SERIALIZABLE"] {
        let outcome = repeatable_read_after_concurrent_commit(level).await?;
        let fault = outcome.err().unwrap_or_else(|| {
            panic!("{level}: a key committed after the snapshot cannot be found")
        });
        assert!(
            fault.is_transient(),
            "{level}: expected a transient fault, got {fault:?}"
        );
        assert!(!fault.is_fatal(), "{level}: {fault:?}");
    }
    Ok(())
}

#[tokio::test]
async fn under_read_committed_finds_a_concurrent_commit() -> anyhow::Result<()> {
    let outcome = repeatable_read_after_concurrent_commit("READ COMMITTED").await?;
    let Ensured::Found(found) = outcome? else {
        panic!("read committed sees the committed row")
    };
    assert_eq!(found.note, "committed elsewhere");
    Ok(())
}
