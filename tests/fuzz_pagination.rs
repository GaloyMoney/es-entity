//! Tier 2 property tests: DB-backed pagination determinism.
//!
//! These exercise the generated `list_by` cursor pagination against a live
//! Postgres. Isolation comes from the scoped `Contacts` repo: each case seeds
//! under a fresh `PartnerId`, so no other (parallel) test's rows leak in and
//! cross-pass comparisons are stable.
//!
//! Two invariants, neither covered by the existing fixed-size pagination tests:
//!   1. **Page-size invariance + completeness**: paginating a fixed scope fully
//!      at page sizes 1, 2, 3 and "large" must yield the identical ordered id
//!      sequence, with no duplicates or gaps, in both directions.
//!   2. **`has_next_page` boundary + cursor resume**: a single page at an
//!      arbitrary `first` returns exactly the expected slice of the full order,
//!      `has_next_page` flips at exactly the right boundary, and resuming from
//!      `end_cursor` continues the sequence.
//!
//! Each proptest case spins up its own current-thread runtime *and* connection
//! pool: sqlx connections are bound to the runtime that created them, so a pool
//! shared across per-case runtimes goes stale (`PoolTimedOut`) when the first
//! runtime is dropped.

mod entities;
mod helpers;

use proptest::prelude::*;
use sqlx::PgPool;

use entities::contact::*;
use es_entity::*;

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Contact",
    columns(
        partner_id(ty = "PartnerId", scope, find_by = true, list_for(by(created_at))),
        email(ty = "String"),
        status(ty = "String", list_for(by(created_at))),
    )
)]
pub struct Contacts {
    pool: PgPool,
}

impl Contacts {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

async fn seed(repo: &Contacts, partner: PartnerId, n: usize) -> Vec<ContactId> {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let id = ContactId::new();
        let new = NewContact::builder()
            .id(id)
            .partner_id(partner)
            .email(format!("f{i}-{id}@x.test"))
            .status("active")
            .build()
            .unwrap();
        repo.create(new).await.expect("create contact");
        ids.push(id);
    }
    ids
}

/// Paginate `list_by_created_at` to exhaustion, returning entity ids in order.
async fn paginate_ids(
    repo: &Contacts,
    partner: PartnerId,
    first: usize,
    dir: ListDirection,
) -> Vec<ContactId> {
    let mut out = Vec::new();
    let mut after = None;
    loop {
        let ret = repo
            .list_by_created_at(partner, PaginatedQueryArgs { first, after }, dir)
            .await
            .expect("list_by_created_at");
        out.extend(ret.entities.iter().map(|c| c.id));
        if !ret.has_next_page {
            break;
        }
        after = ret.end_cursor;
        assert!(after.is_some(), "has_next_page without end_cursor");
    }
    out
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 20, failure_persistence: None, ..ProptestConfig::default() })]

    /// Full pagination of a scope is invariant under page size, complete, and
    /// duplicate-free — in both directions.
    #[test]
    fn pagination_is_page_size_invariant_and_complete(n in 0u8..24u8) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async move {
            let pool = helpers::init_pool().await.expect("pool");
            let repo = Contacts::new(pool);
            let partner = PartnerId::new();
            let seeded = seed(&repo, partner, n as usize).await;
            let seeded_set: std::collections::HashSet<ContactId> = seeded.iter().copied().collect();

            for dir in [ListDirection::Ascending, ListDirection::Descending] {
                let large = paginate_ids(&repo, partner, (n as usize) + 5, dir).await;
                let by1 = paginate_ids(&repo, partner, 1, dir).await;
                let by2 = paginate_ids(&repo, partner, 2, dir).await;
                let by3 = paginate_ids(&repo, partner, 3, dir).await;

                // completeness + no duplicates
                prop_assert_eq!(large.len(), seeded.len());
                let large_set: std::collections::HashSet<ContactId> = large.iter().copied().collect();
                prop_assert_eq!(large_set.len(), large.len(), "duplicate ids in pagination");
                prop_assert_eq!(&large_set, &seeded_set, "pagination missed/added rows");

                // page-size invariance: every page size yields the identical order
                prop_assert_eq!(&by1, &large, "page size 1 diverged ({:?})", dir);
                prop_assert_eq!(&by2, &large, "page size 2 diverged ({:?})", dir);
                prop_assert_eq!(&by3, &large, "page size 3 diverged ({:?})", dir);
            }
            Ok(())
        })?;
    }

    /// A single page at arbitrary `first` returns the right slice, `has_next_page`
    /// flips at the exact boundary, and resuming from `end_cursor` continues.
    #[test]
    fn has_next_page_boundary_and_cursor_resume(n in 1u8..20u8, first in 1u8..23u8) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async move {
            let pool = helpers::init_pool().await.expect("pool");
            let repo = Contacts::new(pool);
            let partner = PartnerId::new();
            let _seeded = seed(&repo, partner, n as usize).await;

            let full = paginate_ids(&repo, partner, (n as usize) + 5, ListDirection::Ascending).await;
            let first = first as usize;
            let n = n as usize;

            // Page 1 from no cursor.
            let p1 = repo
                .list_by_created_at(partner, PaginatedQueryArgs { first, after: None }, ListDirection::Ascending)
                .await
                .expect("page 1");
            let want1: Vec<ContactId> = full.iter().take(first).copied().collect();
            let got1: Vec<ContactId> = p1.entities.iter().map(|c| c.id).collect();
            prop_assert_eq!(&got1, &want1, "page 1 slice");
            prop_assert_eq!(p1.has_next_page, n > first, "page 1 has_next_page");

            // Resume from end_cursor if there is more.
            if p1.has_next_page {
                let cursor = p1.end_cursor.expect("has_next_page without end_cursor");
                let p2 = repo
                    .list_by_created_at(partner, PaginatedQueryArgs { first, after: Some(cursor) }, ListDirection::Ascending)
                    .await
                    .expect("page 2");
                let want2: Vec<ContactId> = full.iter().skip(first).take(first).copied().collect();
                let got2: Vec<ContactId> = p2.entities.iter().map(|c| c.id).collect();
                prop_assert_eq!(&got2, &want2, "page 2 (resumed) slice");
                prop_assert_eq!(p2.has_next_page, n > 2 * first, "page 2 has_next_page");
            }
            Ok(())
        })?;
    }
}
