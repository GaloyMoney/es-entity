mod helpers;

use std::collections::BTreeSet;

use es_entity::errlanes::Fail;
use es_entity::*;
use serde::{Deserialize, Serialize};
use sqlx::PgPool;

es_entity::entity_id! { EnsureSoftMemberId }

#[derive(EsEvent, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[es_event(id = "EnsureSoftMemberId")]
pub enum EnsureSoftMemberEvent {
    Initialized {
        id: EnsureSoftMemberId,
        email: String,
        handle: String,
    },
}

#[derive(EsEntity)]
pub struct EnsureSoftMember {
    pub id: EnsureSoftMemberId,
    pub email: String,
    pub handle: String,
    events: EntityEvents<EnsureSoftMemberEvent>,
}

impl TryFromEvents<EnsureSoftMemberEvent> for EnsureSoftMember {
    fn try_from_events(
        events: EntityEvents<EnsureSoftMemberEvent>,
    ) -> Result<Self, EntityHydrationError> {
        let (id, email, handle) = events
            .iter_all()
            .map(|EnsureSoftMemberEvent::Initialized { id, email, handle }| {
                (*id, email.clone(), handle.clone())
            })
            .next()
            .expect("an initialized event");
        Ok(EnsureSoftMember {
            id,
            email,
            handle,
            events,
        })
    }
}

pub struct NewEnsureSoftMember {
    pub id: EnsureSoftMemberId,
    pub email: String,
    pub handle: String,
}

impl IntoEvents<EnsureSoftMemberEvent> for NewEnsureSoftMember {
    type IdSource = FromCaller;

    fn into_events(
        self,
        builder: EntityEventsBuilder<EnsureSoftMemberEvent, FromCaller>,
    ) -> EntityEvents<EnsureSoftMemberEvent> {
        builder.initial_events(
            self.id,
            [EnsureSoftMemberEvent::Initialized {
                id: self.id,
                email: self.email,
                handle: self.handle,
            }],
        )
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "EnsureSoftMember",
    id(source = caller),
    delete = "soft",
    columns(email(ty = "String"), handle(ty = "String"))
)]
pub struct EnsureSoftMembers {
    pool: PgPool,
}

mod without_queries {
    use es_entity::*;
    use sqlx::PgPool;

    use super::{EnsureSoftMember, EnsureSoftMemberEvent, EnsureSoftMemberId};

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "EnsureSoftMember",
        id(source = caller),
        delete = "soft_without_queries",
        columns(email(ty = "String"), handle(ty = "String"))
    )]
    pub struct EnsureBareSoftMembers {
        pub pool: PgPool,
    }
}

fn unique(prefix: &str) -> String {
    format!("{prefix}-{}", EnsureSoftMemberId::new())
}

fn member(id: EnsureSoftMemberId, email: &str, handle: &str) -> NewEnsureSoftMember {
    NewEnsureSoftMember {
        id,
        email: email.to_string(),
        handle: handle.to_string(),
    }
}

async fn event_count(pool: &PgPool, id: EnsureSoftMemberId) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM ensure_soft_member_events WHERE id = $1")
            .bind(uuid::Uuid::from(id))
            .fetch_one(pool)
            .await?,
    )
}

async fn row_count(pool: &PgPool, email: &str) -> anyhow::Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT COUNT(*) FROM ensure_soft_members WHERE email = $1")
            .bind(email)
            .fetch_one(pool)
            .await?,
    )
}

async fn created_and_deleted(
    repo: &EnsureSoftMembers,
    op: &mut DbOp<'_>,
    email: &str,
    handle: &str,
) -> anyhow::Result<EnsureSoftMemberId> {
    let id = EnsureSoftMemberId::new();
    let created = repo
        .create_in_op(&mut *op, member(id, email, handle))
        .await?;
    repo.delete_in_op(&mut *op, created).await?;
    Ok(id)
}

#[tokio::test]
async fn found_deleted_through_caller_id() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftMembers { pool: pool.clone() };
    let email = unique("email");

    let mut op = DbOp::init(&pool).await?;
    let id = created_and_deleted(&repo, &mut op, &email, &unique("handle")).await?;
    op.commit().await?;
    let events_before = event_count(&pool, id).await?;
    assert_eq!(events_before, 1);

    let mut op = DbOp::init(&pool).await?;
    let ensured = repo
        .create_or_find_in_op(
            &mut op,
            member(id, &unique("other-email"), &unique("other-handle")),
        )
        .await?;
    assert!(ensured.is_found_deleted());
    let EnsuredOrDeleted::FoundDeleted(found) = ensured else {
        panic!("expected FoundDeleted");
    };
    assert_eq!(found.id, id);
    assert_eq!(found.email, email);
    op.commit().await?;

    assert_eq!(event_count(&pool, id).await?, events_before);
    assert_eq!(row_count(&pool, &email).await?, 1);
    Ok(())
}

#[tokio::test]
async fn found_deleted_through_full_unique_key() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftMembers { pool: pool.clone() };
    let email = unique("email");

    let mut op = DbOp::init(&pool).await?;
    let dead = created_and_deleted(&repo, &mut op, &email, &unique("handle")).await?;

    let ensured = repo
        .create_or_find_in_op(
            &mut op,
            member(EnsureSoftMemberId::new(), &email, &unique("handle")),
        )
        .await?;
    let EnsuredOrDeleted::FoundDeleted(found) = ensured else {
        panic!("expected FoundDeleted");
    };
    assert_eq!(found.id, dead);
    assert!(EnsuredOrDeleted::FoundDeleted(found).into_live().is_none());
    Ok(())
}

#[tokio::test]
async fn deleted_row_does_not_hold_the_live_partial_key() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftMembers { pool: pool.clone() };
    let handle = unique("handle");

    let mut op = DbOp::init(&pool).await?;
    created_and_deleted(&repo, &mut op, &unique("email"), &handle).await?;

    let id = EnsureSoftMemberId::new();
    let ensured = repo
        .create_or_find_in_op(&mut op, member(id, &unique("email"), &handle))
        .await?;
    assert!(ensured.is_created());
    assert_eq!(ensured.into_live().expect("created is live").id, id);
    Ok(())
}

#[tokio::test]
async fn live_duplicate_on_the_promoted_partial_key_is_found() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftMembers { pool: pool.clone() };
    let handle = unique("handle");
    let holder = EnsureSoftMemberId::new();

    let mut op = DbOp::init(&pool).await?;
    repo.create_in_op(&mut op, member(holder, &unique("email"), &handle))
        .await?;

    let ensured = repo
        .create_or_find_in_op(
            &mut op,
            member(EnsureSoftMemberId::new(), &unique("email"), &handle),
        )
        .await?;
    let EnsuredOrDeleted::Found(found) = ensured else {
        panic!("expected Found");
    };
    assert_eq!(found.id, holder);
    Ok(())
}

#[tokio::test]
async fn live_match_wins_over_deleted_match() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftMembers { pool: pool.clone() };
    let email = unique("email");
    let live = EnsureSoftMemberId::new();

    let mut op = DbOp::init(&pool).await?;
    let dead = created_and_deleted(&repo, &mut op, &unique("email"), &unique("handle")).await?;
    repo.create_in_op(&mut op, member(live, &email, &unique("handle")))
        .await?;

    let ensured = repo
        .create_or_find_in_op(&mut op, member(dead, &email, &unique("handle")))
        .await?;
    let EnsuredOrDeleted::Found(found) = ensured else {
        panic!("expected Found");
    };
    assert_eq!(found.id, live);
    Ok(())
}

#[tokio::test]
async fn two_deleted_matches_span() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftMembers { pool: pool.clone() };
    let email = unique("email");

    let mut op = DbOp::init(&pool).await?;
    let x = created_and_deleted(&repo, &mut op, &unique("email"), &unique("handle")).await?;
    let y = created_and_deleted(&repo, &mut op, &email, &unique("handle")).await?;

    let err = repo
        .create_or_find_in_op(&mut op, member(x, &email, &unique("handle")))
        .await
        .err()
        .expect("two deleted holders must reject");
    let Fail::Rejected(rejection) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    let EnsureSoftMemberEnsureRejection::KeysSpanEntities { ids, keys } = rejection;
    assert_eq!(
        ids.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([x, y])
    );
    assert_eq!(
        keys.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from(["id", "email"])
    );

    repo.create_in_op(
        &mut op,
        member(
            EnsureSoftMemberId::new(),
            &unique("email"),
            &unique("handle"),
        ),
    )
    .await?;
    Ok(())
}

#[tokio::test]
async fn two_live_matches_span() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftMembers { pool: pool.clone() };
    let x = EnsureSoftMemberId::new();
    let y = EnsureSoftMemberId::new();
    let email = unique("email");

    let mut op = DbOp::init(&pool).await?;
    repo.create_in_op(&mut op, member(x, &unique("email"), &unique("handle")))
        .await?;
    repo.create_in_op(&mut op, member(y, &email, &unique("handle")))
        .await?;

    let err = repo
        .create_or_find_in_op(&mut op, member(x, &email, &unique("handle")))
        .await
        .err()
        .expect("two live holders must reject");
    let Fail::Rejected(EnsureSoftMemberEnsureRejection::KeysSpanEntities { ids, .. }) = err else {
        panic!("expected a rejection, got {err:?}");
    };
    assert_eq!(
        ids.into_iter().collect::<BTreeSet<_>>(),
        BTreeSet::from([x, y])
    );
    Ok(())
}

#[tokio::test]
async fn created_when_nothing_holds_a_key() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = EnsureSoftMembers { pool: pool.clone() };
    let id = EnsureSoftMemberId::new();

    let mut op = DbOp::init(&pool).await?;
    let ensured = repo
        .create_or_find_in_op(&mut op, member(id, &unique("email"), &unique("handle")))
        .await?;
    assert!(ensured.is_created());
    op.commit().await?;
    assert_eq!(event_count(&pool, id).await?, 1);
    Ok(())
}

#[tokio::test]
async fn soft_without_queries_repo_has_the_method() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let repo = without_queries::EnsureBareSoftMembers { pool: pool.clone() };
    let email = unique("email");
    let id = EnsureSoftMemberId::new();

    let mut op = DbOp::init(&repo.pool).await?;
    let created = repo
        .create_or_find_in_op(&mut op, member(id, &email, &unique("handle")))
        .await?;
    assert!(created.is_created());
    sqlx::query("UPDATE ensure_soft_members SET deleted = TRUE WHERE id = $1")
        .bind(uuid::Uuid::from(id))
        .execute(op.as_executor())
        .await?;

    let ensured = repo
        .create_or_find_in_op(
            &mut op,
            member(EnsureSoftMemberId::new(), &email, &unique("handle")),
        )
        .await?;
    let EnsuredOrDeleted::FoundDeleted(found) = ensured else {
        panic!("expected FoundDeleted");
    };
    assert_eq!(found.id, id);
    Ok(())
}
