mod entities;
mod helpers;

use entities::{order::*, user::*};
use es_entity::*;
use sqlx::PgPool;

/// A repo whose only non-id column never persists on update, and which has
/// no nested children: `update_can_reject()` is false for it, so
/// `update`/`update_in_op` narrow to the plain `RepoFault` instead of
/// `RepoWriteError<CV>` — there is nothing left in the `Rejected` lane for
/// them to ever construct. The real motivating case is a repo of
/// scope/reference columns written once on create and never again.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "User",
    tbl = "users",
    columns(name(ty = "String", update(persist = false)))
)]
pub struct ImmutableNameUsers {
    pool: PgPool,
}

impl ImmutableNameUsers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

/// Signature-level proof: this only compiles if `update`'s generated
/// signature actually narrowed to `RepoFault` rather than
/// `RepoWriteError<ImmutableNameUsersConstraintViolation>`.
fn takes_fault(res: Result<usize, es_entity::RepoFault>) -> Result<usize, es_entity::RepoFault> {
    res
}

#[tokio::test]
async fn update_without_a_constrained_column_or_nesting_returns_repo_fault() -> anyhow::Result<()> {
    let repo = ImmutableNameUsers::new(helpers::init_pool().await?);

    let mut user = repo
        .create(
            NewUser::builder()
                .id(UserId::new())
                .name("First")
                .build()
                .unwrap(),
        )
        .await?;
    let _ = user.update_name("Second");

    let n_events = takes_fault(repo.update(&mut user).await)?;
    assert_eq!(n_events, 1);

    Ok(())
}

/// A stale-sequence update still surfaces `Fault::Transient(OptimisticConflict)`
/// through the narrower `RepoFault` type — narrowing drops the `Rejected`
/// lane only, not `Transient`/`Fatal`.
#[tokio::test]
async fn stale_sequence_update_is_transient_optimistic_conflict_through_repo_fault()
-> anyhow::Result<()> {
    let repo = ImmutableNameUsers::new(helpers::init_pool().await?);

    let user = repo
        .create(
            NewUser::builder()
                .id(UserId::new())
                .name("Concurrent")
                .build()
                .unwrap(),
        )
        .await?;

    let mut first = repo.find_by_id(user.id).await?;
    let mut second = repo.find_by_id(user.id).await?;

    let _ = first.update_name("first_writer");
    repo.update(&mut first).await?;

    let _ = second.update_name("second_writer");
    match repo.update(&mut second).await {
        Err(es_entity::Fault::Transient(t)) => {
            assert_eq!(t.kind, es_entity::TransientKind::OptimisticConflict);
        }
        other => panic!("expected Fault::Transient(OptimisticConflict), got: {other:?}"),
    }

    Ok(())
}

/// The only constrained column `update` persists is the `order_id` foreign
/// key. A foreign key violation is an invariant, never a rejection, so
/// `update_can_reject()` is false and `update`/`update_in_op` narrow to
/// `RepoFault` even though the combined index+events statement is used.
#[derive(EsRepo, Debug)]
#[es_repo(entity = "OrderItem", columns(order_id(ty = "OrderId")))]
pub struct ForeignKeyOnlyOrderItems {
    pool: PgPool,
}

impl ForeignKeyOnlyOrderItems {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn update_persisting_only_a_foreign_key_column_returns_repo_fault() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let order_id = OrderId::new();
    sqlx::query!(
        "INSERT INTO orders (id, created_at) VALUES ($1, now())",
        order_id as OrderId
    )
    .execute(&pool)
    .await?;
    let repo = ForeignKeyOnlyOrderItems::new(pool);

    let mut item = repo
        .create(
            NewOrderItem::builder()
                .id(OrderItemId::new())
                .order_id(order_id)
                .product_name("Widget")
                .quantity(1)
                .price(1.0)
                .build()
                .unwrap(),
        )
        .await?;
    let _ = item.update_quantity(2);

    let n_events = takes_fault(repo.update(&mut item).await)?;
    assert_eq!(n_events, 1);

    Ok(())
}
