mod helpers;

use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use es_entity::errlanes::{Fail, RejectionCode};
use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! { EnsureTemplateId, EnsureSlotId }

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
async fn ensure_by_unique_key_creates_then_finds() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureTemplates { pool: pool.clone() };
    let code = unique("seq");

    let mut op = DbOp::init(&pool).await?;
    let first = takes_fault(
        repo.create_or_find_by_code_in_op(&mut op, template(&code, "first"))
            .await,
    )?;
    let second = takes_fault(
        repo.create_or_find_by_code_in_op(&mut op, template(&code, "second"))
            .await,
    )?;
    let other = takes_fault(
        repo.create_or_find_by_code_in_op(&mut op, template(&unique("other"), "third"))
            .await,
    )?;
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
async fn ensure_found_does_not_persist_new_events_or_run_hook() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureTemplates { pool: pool.clone() };
    let code = unique("hook");

    let mut op = DbOp::init(&pool).await?;
    let before_create = HOOK_RUNS.load(Ordering::SeqCst);
    let first = takes_fault(
        repo.create_or_find_by_code_in_op(&mut op, template(&code, "kept"))
            .await,
    )?;
    let after_create = HOOK_RUNS.load(Ordering::SeqCst);
    let second = takes_fault(
        repo.create_or_find_by_code_in_op(&mut op, template(&code, "dropped"))
            .await,
    )?;
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
async fn ensure_by_unique_key_concurrent_ops_one_created_one_found() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let code = unique("concurrent");

    let repo = EnsureTemplates { pool: pool.clone() };
    let mut first_op = DbOp::init(&pool).await?;
    let first = takes_fault(
        repo.create_or_find_by_code_in_op(&mut first_op, template(&code, "winner"))
            .await,
    )?;

    let second_pool = pool.clone();
    let second_code = code.clone();
    let second = tokio::spawn(async move {
        let repo = EnsureTemplates {
            pool: second_pool.clone(),
        };
        let mut op = DbOp::init(&second_pool).await?;
        let ensured = repo
            .create_or_find_by_code_in_op(&mut op, template(&second_code, "loser"))
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
async fn ensure_by_unique_key_rolled_back_holder_lets_the_blocked_op_create() -> anyhow::Result<()>
{
    let pool = helpers::init_pool().await?;
    let code = unique("rollback");

    let repo = EnsureTemplates { pool: pool.clone() };
    let mut first_op = DbOp::init(&pool).await?;
    takes_fault(
        repo.create_or_find_by_code_in_op(&mut first_op, template(&code, "abandoned"))
            .await,
    )?;

    let second_pool = pool.clone();
    let second_code = code.clone();
    let second = tokio::spawn(async move {
        let repo = EnsureTemplates {
            pool: second_pool.clone(),
        };
        let mut op = DbOp::init(&second_pool).await?;
        let ensured = repo
            .create_or_find_by_code_in_op(&mut op, template(&second_code, "kept"))
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
async fn ensure_by_unique_key_many_concurrent_ops_create_exactly_once() -> anyhow::Result<()> {
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
                .create_or_find_by_code_in_op(&mut op, template(&code, &format!("n{i}")))
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
async fn ensure_residual_empty_is_repo_fault() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureTemplates { pool: pool.clone() };
    let mut op = DbOp::init(&pool).await?;
    let ensured: Result<Ensured<EnsureTemplate>, RepoFault> = repo
        .create_or_find_by_code_in_op(&mut op, template(&unique("empty"), "n"))
        .await;
    assert!(ensured?.is_created());
    Ok(())
}

#[tokio::test]
async fn ensure_by_composite_key() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSlots { pool: pool.clone() };
    let account = unique("account");

    let mut op = DbOp::init(&pool).await?;
    let first = repo
        .create_or_find_by_account_and_currency_in_op(
            &mut op,
            slot(&account, "USD", &unique("label")),
        )
        .await?;
    let second = repo
        .create_or_find_by_account_and_currency_in_op(
            &mut op,
            slot(&account, "USD", &unique("label")),
        )
        .await?;
    let other_currency = repo
        .create_or_find_by_account_and_currency_in_op(
            &mut op,
            slot(&account, "EUR", &unique("label")),
        )
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
async fn ensure_residual_has_no_ensured_key_member() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSlots { pool: pool.clone() };
    let label = unique("label");

    let mut op = DbOp::init(&pool).await?;
    repo.create_or_find_by_account_and_currency_in_op(
        &mut op,
        slot(&unique("account"), "USD", &label),
    )
    .await?;
    op.commit().await?;

    let mut op = DbOp::init(&pool).await?;
    let err = repo
        .create_or_find_by_account_and_currency_in_op(
            &mut op,
            slot(&unique("account"), "USD", &label),
        )
        .await
        .err()
        .expect("a taken label must reject");
    let Fail::Rejected(rejection) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    match rejection {
        EnsureSlotEnsureByAccountAndCurrencyRejection::LabelKey(_) => {}
    }
    assert_eq!(
        EnsureSlotEnsureByAccountAndCurrencyRejectionCode::CODES.len(),
        1
    );

    let mut op = DbOp::init(&pool).await?;
    let account = unique("account");
    repo.create_or_find_by_label_in_op(&mut op, slot(&account, "USD", &unique("label")))
        .await?;
    let err = repo
        .create_or_find_by_label_in_op(&mut op, slot(&account, "USD", &unique("label")))
        .await
        .err()
        .expect("a taken account and currency must reject");
    let Fail::Rejected(rejection) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    match rejection {
        EnsureSlotEnsureByLabelRejection::AccountCurrencyKey(_) => {}
    }
    assert_eq!(EnsureSlotEnsureByLabelRejectionCode::CODES.len(), 1);
    Ok(())
}

#[tokio::test]
async fn ensure_rejections_carry_names_not_values() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSlots { pool: pool.clone() };
    let secret = unique("secret-label");

    let mut op = DbOp::init(&pool).await?;
    repo.create_or_find_by_account_and_currency_in_op(
        &mut op,
        slot(&unique("account"), "USD", &secret),
    )
    .await?;
    op.commit().await?;

    let mut op = DbOp::init(&pool).await?;
    let err = repo
        .create_or_find_by_account_and_currency_in_op(
            &mut op,
            slot(&unique("account"), "USD", &secret),
        )
        .await
        .err()
        .expect("a taken label must reject");
    let Fail::Rejected(rejection) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    assert_eq!(rejection.constraint_name(), "ensure_slots_label_key");
    assert!(!rejection.to_string().contains(&secret));
    Ok(())
}

async fn repeatable_read_after_concurrent_commit(
    level: &str,
) -> anyhow::Result<Result<Ensured<EnsureTemplate>, RepoFault>> {
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
    repo.create_or_find_by_code_in_op(&mut other, template(&code, "committed elsewhere"))
        .await?;
    other.commit().await?;

    Ok(repo
        .create_or_find_by_code_in_op(&mut tx, template(&code, "late"))
        .await)
}

#[tokio::test]
async fn ensure_under_snapshot_isolation_is_transient_never_corrupt() -> anyhow::Result<()> {
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
async fn ensure_under_read_committed_finds_a_concurrent_commit() -> anyhow::Result<()> {
    let outcome = repeatable_read_after_concurrent_commit("READ COMMITTED").await?;
    let Ensured::Found(found) = outcome? else {
        panic!("read committed sees the committed row")
    };
    assert_eq!(found.note, "committed elsewhere");
    Ok(())
}
