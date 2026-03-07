mod entities;
mod helpers;

use entities::user::*;

// ---------------------------------------------------------------------------
// Custom error types for the hooks
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub struct UserHydrateValidationError(String);

impl std::fmt::Display for UserHydrateValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "post-hydrate validation failed: {}", self.0)
    }
}

impl std::error::Error for UserHydrateValidationError {}

#[derive(Debug)]
pub struct UserPersistAuditError(String);

impl std::fmt::Display for UserPersistAuditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "post-persist audit failed: {}", self.0)
    }
}

impl std::error::Error for UserPersistAuditError {}

// ---------------------------------------------------------------------------
// Repo with post_hydrate_hook — rejects entities whose name is "BANNED"
// ---------------------------------------------------------------------------

mod users_with_hydrate_hook {
    use es_entity::*;
    use sqlx::PgPool;

    use crate::UserHydrateValidationError;
    use crate::entities::user::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "User",
        columns(name = "String"),
        post_hydrate_hook(method = "validate_hydrated", error = "UserHydrateValidationError")
    )]
    pub struct UsersWithHydrateHook {
        pool: PgPool,
    }

    impl UsersWithHydrateHook {
        pub fn new(pool: PgPool) -> Self {
            Self { pool }
        }

        fn validate_hydrated(&self, entity: &User) -> Result<(), UserHydrateValidationError> {
            if entity.name == "BANNED" {
                Err(UserHydrateValidationError(format!(
                    "user '{}' has a banned name",
                    entity.id
                )))
            } else {
                Ok(())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Repo with post_persist_hook (new syntax) — rejects persisting name "BLOCKED"
// ---------------------------------------------------------------------------

mod users_with_persist_hook {
    use es_entity::*;
    use sqlx::PgPool;

    use crate::UserPersistAuditError;
    use crate::entities::user::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        entity = "User",
        columns(name = "String"),
        post_persist_hook(method = "audit_persist", error = "UserPersistAuditError")
    )]
    pub struct UsersWithPersistHook {
        pool: PgPool,
    }

    impl UsersWithPersistHook {
        pub fn new(pool: PgPool) -> Self {
            Self { pool }
        }

        async fn audit_persist<OP: es_entity::AtomicOperation>(
            &self,
            _op: &mut OP,
            entity: &User,
            _new_events: es_entity::events::LastPersisted<'_, UserEvent>,
        ) -> Result<(), UserPersistAuditError> {
            if entity.name == "BLOCKED" {
                Err(UserPersistAuditError(format!(
                    "cannot persist user '{}' with blocked name",
                    entity.id
                )))
            } else {
                Ok(())
            }
        }
    }
}

use users_with_hydrate_hook::UsersWithHydrateHook;
use users_with_persist_hook::UsersWithPersistHook;

// ===========================================================================
// post_hydrate_hook tests
// ===========================================================================

#[tokio::test]
async fn post_hydrate_hook_error_propagates_through_create() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = UsersWithHydrateHook::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("BANNED")
        .build()
        .unwrap();

    let result = users.create(new_user).await;

    match result {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("PostHydrateError"),
                "expected PostHydrateError, got: {msg}"
            );
        }
        Ok(_) => panic!("expected post_hydrate_hook to reject entity with banned name"),
    }

    Ok(())
}

#[tokio::test]
async fn post_hydrate_hook_allows_valid_entities() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = UsersWithHydrateHook::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Alice")
        .build()
        .unwrap();

    let user = users.create(new_user).await?;

    let loaded = users.find_by_id(user.id).await?;
    assert_eq!(loaded.name, "Alice");

    Ok(())
}

#[tokio::test]
async fn post_hydrate_hook_error_propagates_through_find_by_id() -> anyhow::Result<()> {
    // First, create a valid user using a plain repo (no hook), then rename to
    // "BANNED" and load through the hook repo.
    let pool = helpers::init_pool().await?;

    // Create via the hook repo with a valid name
    let users = UsersWithHydrateHook::new(pool.clone());
    let id = UserId::new();
    let new_user = NewUser::builder()
        .id(id)
        .name("ValidAtFirst")
        .build()
        .unwrap();
    let mut user = users.create(new_user).await?;

    // Update the user's name to the banned value
    let _ = user.update_name("BANNED");
    users.update(&mut user).await?;

    // Now find_by_id should fail with the hydration hook error
    let result = users.find_by_id(id).await;
    match result {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("PostHydrateError"),
                "expected PostHydrateError in find_by_id, got: {msg}"
            );
        }
        Ok(_) => panic!("expected post_hydrate_hook to reject entity loaded with banned name"),
    }

    Ok(())
}

// ===========================================================================
// post_persist_hook tests
// ===========================================================================

#[tokio::test]
async fn post_persist_hook_error_propagates_through_create() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = UsersWithPersistHook::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("BLOCKED")
        .build()
        .unwrap();

    let result = users.create(new_user).await;

    match result {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("PostPersistHookError"),
                "expected PostPersistHookError, got: {msg}"
            );
        }
        Ok(_) => panic!("expected post_persist_hook to reject entity with blocked name"),
    }

    Ok(())
}

#[tokio::test]
async fn post_persist_hook_allows_valid_entities() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = UsersWithPersistHook::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Bob")
        .build()
        .unwrap();

    let user = users.create(new_user).await?;
    assert_eq!(user.name, "Bob");

    Ok(())
}

#[tokio::test]
async fn post_persist_hook_error_propagates_through_update() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = UsersWithPersistHook::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Carol")
        .build()
        .unwrap();
    let mut user = users.create(new_user).await?;

    // Rename to blocked value and update
    let _ = user.update_name("BLOCKED");
    let result = users.update(&mut user).await;

    match result {
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("PostPersistHookError"),
                "expected PostPersistHookError in update, got: {msg}"
            );
        }
        Ok(_) => panic!("expected post_persist_hook to reject update to blocked name"),
    }

    Ok(())
}
