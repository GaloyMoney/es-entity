//! `#[es_repo(in_op_only)]`: only the `_in_op` variants of the repo fns are
//! generated, so there is no way to reach the database without passing an
//! operation. With no standalone fn left to open one, the pool field itself
//! becomes optional — the repo below holds nothing at all.
//!
//! The *enforcement* half of this property (that calling a non-`_in_op` form
//! fails to compile) is pinned by a `compile_fail` doctest on
//! `es_entity::EsRepo`, and by token-level assertions in
//! `es-entity-macros/src/repo/mod.rs`. What this file proves is the other
//! half: that the surviving `_in_op`-only surface actually works end to end
//! against Postgres, pool-less.

mod entities;
mod helpers;

use entities::user::*;
use es_entity::{DbOp, *};

/// A repo with no fields whatsoever: no pool, no clock. Every fn it exposes
/// takes the operation from its caller.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "User",
    in_op_only,
    columns(name(ty = "String", list_by, list_for))
)]
pub struct Users {}

fn new_user(name: &str) -> NewUser {
    NewUser::builder()
        .id(UserId::new())
        .name(name.to_string())
        .build()
        .expect("failed to build user")
}

#[tokio::test]
async fn pool_less_repo_writes_and_reads_through_a_passed_op() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users {};

    // Writes: the caller owns the transaction from beginning to commit.
    let mut op = DbOp::init(&pool).await?;
    let mut user = users
        .create_in_op(&mut op, new_user("in_op_only create"))
        .await?;
    let id = user.id;
    op.commit().await?;

    let mut op = DbOp::init(&pool).await?;
    let _ = user.update_name("in_op_only updated");
    users.update_in_op(&mut op, &mut user).await?;
    op.commit().await?;

    // Reads take a one-shot executor. A borrowed pool is one...
    let found = users.find_by_id_in_op(&pool, id).await?;
    assert_eq!(found.name, "in_op_only updated");

    // ...and so is a live operation, so a read can observe uncommitted state
    // written earlier in the same transaction.
    let mut op = DbOp::init(&pool).await?;
    let mut uncommitted = users.create_in_op(&mut op, new_user("uncommitted")).await?;
    let uncommitted_id = uncommitted.id;
    let seen = users.find_by_id_in_op(&mut op, uncommitted_id).await?;
    assert_eq!(seen.name, "uncommitted");
    let _ = uncommitted.update_name("still uncommitted");
    users.update_in_op(&mut op, &mut uncommitted).await?;
    op.commit().await?;

    Ok(())
}

#[tokio::test]
async fn pool_less_repo_serves_every_read_family_in_op() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users {};

    let unique = format!("in_op_only_reads_{}", UserId::new());
    let mut op = DbOp::init(&pool).await?;
    let user = users.create_in_op(&mut op, new_user(&unique)).await?;
    op.commit().await?;

    // find_all
    let all: std::collections::HashMap<UserId, User> =
        users.find_all_in_op(&pool, &[user.id]).await?;
    assert!(all.contains_key(&user.id));

    // list_by_<column>
    let by_name = users
        .list_by_name_in_op(
            &pool,
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
            ListDirection::Descending,
        )
        .await?;
    assert!(!by_name.entities.is_empty());

    // list_for_<column>_by_<column>
    let for_name = users
        .list_for_name_by_id_in_op(
            &pool,
            unique.clone(),
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
            ListDirection::Ascending,
        )
        .await?;
    assert_eq!(for_name.entities.len(), 1);
    assert_eq!(for_name.entities[0].id, user.id);

    // The `list_for_filters` dispatcher, which had no `_in_op` twin before
    // this option existed. Filtered: routes to the single-filter sibling.
    let filtered = users
        .list_for_filters_in_op(
            &pool,
            UserFilters {
                name: Some(unique.clone()),
            },
            Sort {
                by: UserSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
        )
        .await?;
    assert_eq!(filtered.entities.len(), 1);
    assert_eq!(filtered.entities[0].id, user.id);

    // Unfiltered: routes to the plain `list_by_*` sibling.
    let unfiltered = users
        .list_for_filters_in_op(
            &pool,
            UserFilters::default(),
            Sort {
                by: UserSortBy::Id,
                direction: ListDirection::Descending,
            },
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
        )
        .await?;
    assert!(!unfiltered.entities.is_empty());

    Ok(())
}

/// `in_op_only` only *permits* dropping the pool. A repo that keeps one still
/// gets `pool()` and `begin_op()` — holding a pool does not weaken the
/// discipline, because the op still has to be passed in explicitly.
mod with_pool {
    use super::{entities::user::*, helpers, new_user};
    use es_entity::*;
    use sqlx::PgPool;

    #[derive(EsRepo, Debug)]
    #[es_repo(entity = "User", in_op_only, columns(name(ty = "String")))]
    pub struct Users {
        pool: PgPool,
    }

    #[tokio::test]
    async fn in_op_only_repo_with_a_pool_can_still_begin_its_own_op() -> anyhow::Result<()> {
        let pool = helpers::init_pool().await?;
        let users = Users { pool: pool.clone() };

        // `begin_op` survives: it is the pool-owning constructor of an op, not
        // a way to skip passing one.
        let mut op = users.begin_op().await?;
        let user = users
            .create_in_op(&mut op, new_user("in_op_only with pool"))
            .await?;
        op.commit().await?;

        let found = users.find_by_id_in_op(users.pool(), user.id).await?;
        assert_eq!(found.name, "in_op_only with pool");

        Ok(())
    }
}
