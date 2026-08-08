//! Tier 2 property tests: DB-backed CRUD round-trip fidelity.
//!
//! Event sourcing's central promise is that an entity reconstructed from its
//! persisted event stream equals the in-memory snapshot. These tests fuzz that
//! round-trip over random update sequences (including consecutive duplicates,
//! which the idempotency guard collapses) against a live Postgres, using the
//! `User` entity (unique ids → natural isolation, no scoping needed).
//!
//! Each proptest case spins up its own current-thread runtime *and* connection
//! pool — see `fuzz_pagination.rs` for why a shared pool can't span per-case
//! runtimes.

mod entities;
mod helpers;

use proptest::prelude::*;
use sqlx::PgPool;

use entities::user::*;
use es_entity::*;

#[derive(EsRepo, Debug)]
#[es_repo(entity = "User", columns(name(ty = "String", list_for)))]
pub struct Users {
    pool: PgPool,
}

impl Users {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 16, failure_persistence: None, ..ProptestConfig::default() })]

    /// create → (random update_name sequence, persisted incrementally) → reload
    /// must reproduce the in-memory snapshot exactly: same final field values
    /// and the same number of persisted events (no phantom/dropped events).
    #[test]
    fn create_update_reload_roundtrips(
        initial in "[a-z]{1,5}",
        updates in proptest::collection::vec("[a-z]{1,5}", 0..12),
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("rt");
        rt.block_on(async move {
            let pool = helpers::init_pool().await.expect("pool");
            let repo = Users::new(pool);
            let id = UserId::new();
            let new = NewUser::builder()
                .id(id)
                .name(initial)
                .build()
                .unwrap();
            let mut user = repo.create(new).await.expect("create");

            for name in &updates {
                // `update_name` is idempotent: consecutive duplicates are no-ops
                // (no event pushed). Only persist when an event was produced.
                if user.update_name(name.clone()).did_execute() {
                    repo.update(&mut user).await.expect("update");
                }
            }

            let expected_name = user.name.clone();
            let expected_len = user.events().len_persisted();

            let reloaded = repo.find_by_id(id).await.expect("find_by_id");

            prop_assert_eq!(&reloaded.id, &id);
            prop_assert_eq!(&reloaded.name, &expected_name, "reloaded name diverged from snapshot");
            prop_assert_eq!(
                &reloaded.events().len_persisted(),
                &expected_len,
                "reloaded event count diverged from snapshot",
            );
            Ok(())
        })?;
    }
}
