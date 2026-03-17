mod entities;
mod helpers;

use entities::user::*;
use es_entity::{clock::*, *};
use sqlx::PgPool;

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

/// A separate module for the clock field repo to avoid type conflicts
mod users_with_clock {
    use es_entity::{EsEntity, EsEvent, EsRepo, clock::ClockHandle};
    use sqlx::PgPool;

    use crate::entities::user::*;

    /// A repo with an optional clock field for testing clock injection
    #[derive(EsRepo, Debug)]
    #[es_repo(entity = "User", columns(name(ty = "String", list_for)))]
    pub struct UsersWithClock {
        pool: PgPool,
        clock: Option<ClockHandle>,
    }

    impl UsersWithClock {
        pub fn new(pool: PgPool) -> Self {
            Self { pool, clock: None }
        }

        pub fn with_clock(pool: PgPool, clock: ClockHandle) -> Self {
            Self {
                pool,
                clock: Some(clock),
            }
        }
    }
}

use users_with_clock::UsersWithClock;

/// A separate module for the required clock field repo
mod users_with_required_clock {
    use es_entity::{EsEntity, EsEvent, EsRepo, clock::ClockHandle};
    use sqlx::PgPool;

    use crate::entities::user::*;

    /// A repo with a required (non-optional) clock field
    #[derive(EsRepo, Debug)]
    #[es_repo(entity = "User", columns(name(ty = "String", list_for)))]
    pub struct UsersWithRequiredClock {
        pool: PgPool,
        clock: ClockHandle,
    }

    impl UsersWithRequiredClock {
        pub fn new(pool: PgPool, clock: ClockHandle) -> Self {
            Self { pool, clock }
        }
    }
}

use users_with_required_clock::UsersWithRequiredClock;

#[tokio::test]
async fn create_with_manual_clock() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    // Create a manual clock at a specific time (truncated to millis for DB compatibility)
    let fixed_time = {
        let t = chrono::Utc::now() - chrono::Duration::days(30);
        chrono::DateTime::from_timestamp_millis(t.timestamp_millis()).unwrap()
    };
    let (clock, _ctrl) = ClockHandle::manual_at(fixed_time);

    // Begin operation with the manual clock
    let mut op = users.begin_op_with_clock(&clock).await?;

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("TimeTest")
        .build()
        .unwrap();

    let user = users.create_in_op(&mut op, new_user).await?;
    let user_id = user.id;
    op.commit().await?;

    // Load the user back and check the recorded_at timestamp
    let loaded_user = users.find_by_id(user_id).await?;
    let recorded_at = loaded_user
        .events()
        .entity_first_persisted_at()
        .expect("should have recorded_at");

    // The recorded_at should match the manual clock's time
    assert_eq!(recorded_at, fixed_time);

    Ok(())
}

#[tokio::test]
async fn create_with_repo_clock_field() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    // Create a manual clock at a specific time (truncated to millis for DB compatibility)
    let fixed_time = {
        let t = chrono::Utc::now() - chrono::Duration::days(60);
        chrono::DateTime::from_timestamp_millis(t.timestamp_millis()).unwrap()
    };
    let (clock, _ctrl) = ClockHandle::manual_at(fixed_time);

    // Create repo with the clock field set
    let users = UsersWithClock::with_clock(pool, clock);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("ClockFieldTest")
        .build()
        .unwrap();

    // Use the simple create() method - it should use the repo's clock field
    let user = users.create(new_user).await?;
    let user_id = user.id;

    // Load the user back and check the recorded_at timestamp
    let loaded_user = users.find_by_id(user_id).await?;
    let recorded_at = loaded_user
        .events()
        .entity_first_persisted_at()
        .expect("should have recorded_at");

    // The recorded_at should match the manual clock's time
    assert_eq!(recorded_at, fixed_time);

    Ok(())
}

#[tokio::test]
async fn create_with_repo_clock_field_none() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    // Create repo without clock (clock = None) - should use global clock
    let users = UsersWithClock::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("NoClockFieldTest")
        .build()
        .unwrap();

    // Use the simple create() method - it should use the global clock (realtime)
    let user = users.create(new_user).await?;
    let user_id = user.id;

    // Load the user back and verify it has a recorded_at timestamp (near current time)
    let loaded_user = users.find_by_id(user_id).await?;
    let recorded_at = loaded_user
        .events()
        .entity_first_persisted_at()
        .expect("should have recorded_at");

    // The recorded_at should be within the last few seconds (using global clock)
    let now = chrono::Utc::now();
    let diff = now.signed_duration_since(recorded_at);
    assert!(
        diff.num_seconds().abs() < 10,
        "recorded_at should be close to now"
    );

    Ok(())
}

#[tokio::test]
async fn create_with_required_clock_field() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    // Create a manual clock at a specific time (truncated to millis for DB compatibility)
    let fixed_time = {
        let t = chrono::Utc::now() - chrono::Duration::days(90);
        chrono::DateTime::from_timestamp_millis(t.timestamp_millis()).unwrap()
    };
    let (clock, _ctrl) = ClockHandle::manual_at(fixed_time);

    // Create repo with the required clock field
    let users = UsersWithRequiredClock::new(pool, clock);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("RequiredClockTest")
        .build()
        .unwrap();

    // Use the simple create() method - it should always use the repo's clock
    let user = users.create(new_user).await?;
    let user_id = user.id;

    // Load the user back and check the recorded_at timestamp
    let loaded_user = users.find_by_id(user_id).await?;
    let recorded_at = loaded_user
        .events()
        .entity_first_persisted_at()
        .expect("should have recorded_at");

    // The recorded_at should match the manual clock's time
    assert_eq!(recorded_at, fixed_time);

    Ok(())
}
