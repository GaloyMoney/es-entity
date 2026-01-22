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
async fn create() -> anyhow::Result<()> {
    let mut ctx = es_entity::EventContext::current();
    ctx.insert("test", &"create").unwrap();
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Frank")
        .build()
        .unwrap();

    let mut user = users.create(new_user).await?;

    if user.update_name("Dweezil").did_execute() {
        ctx.insert("test", &"update").unwrap();
        users.update(&mut user).await?;
    }

    let loaded_user = users.find_by_id(user.id).await?;

    assert_eq!(user.name, loaded_user.name);

    Ok(())
}

#[tokio::test]
async fn list_by() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Frank")
        .build()
        .unwrap();

    users.create(new_user).await?;
    let PaginatedQueryRet {
        entities,
        has_next_page: _,
        end_cursor: _,
    } = users
        .list_by_id(
            PaginatedQueryArgs {
                first: 5,
                after: Some(user_cursor::UsersByIdCursor {
                    id: uuid::Uuid::nil().into(),
                }),
            },
            ListDirection::Ascending,
        )
        .await?;
    assert!(!entities.is_empty());
    Ok(())
}

#[tokio::test]
async fn list_for_filter() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);

    let PaginatedQueryRet {
        entities,
        has_next_page: _,
        end_cursor: _,
    } = users
        .list_for_filter(
            UsersFilter::NoFilter,
            Sort {
                by: UsersSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
        )
        .await?;

    assert!(!entities.is_empty());

    // Create a user with name Alice for testing the filter
    let alice_id = UserId::new();
    let new_alice = NewUser::builder()
        .id(alice_id)
        .name("Alice")
        .build()
        .unwrap();

    users.create(new_alice).await?;

    let filtered_result = users
        .list_for_filter(
            UsersFilter::WithName("Alice".to_string()),
            Sort {
                by: UsersSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 10,
                after: None,
            },
        )
        .await?;

    assert!(!filtered_result.entities.is_empty(),);
    for user in &filtered_result.entities {
        assert_eq!(user.name, "Alice",);
    }

    Ok(())
}

#[tokio::test]
async fn create_with_artificial_clock() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    // Create an artificial clock at a specific time (truncated to millis for DB compatibility)
    let fixed_time = {
        let t = chrono::Utc::now() - chrono::Duration::days(30);
        chrono::DateTime::from_timestamp_millis(t.timestamp_millis()).unwrap()
    };
    let config = ArtificialClockConfig {
        start_at: fixed_time,
        mode: ArtificialMode::Manual,
    };
    let (clock, _ctrl) = ClockHandle::artificial(config);

    // Begin operation with the artificial clock
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

    // The recorded_at should match the artificial clock's time
    assert_eq!(recorded_at, fixed_time);

    Ok(())
}

#[tokio::test]
async fn create_with_repo_clock_field() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    // Create an artificial clock at a specific time (truncated to millis for DB compatibility)
    let fixed_time = {
        let t = chrono::Utc::now() - chrono::Duration::days(60);
        chrono::DateTime::from_timestamp_millis(t.timestamp_millis()).unwrap()
    };
    let config = ArtificialClockConfig {
        start_at: fixed_time,
        mode: ArtificialMode::Manual,
    };
    let (clock, _ctrl) = ClockHandle::artificial(config);

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

    // The recorded_at should match the artificial clock's time
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

    // Create an artificial clock at a specific time (truncated to millis for DB compatibility)
    let fixed_time = {
        let t = chrono::Utc::now() - chrono::Duration::days(90);
        chrono::DateTime::from_timestamp_millis(t.timestamp_millis()).unwrap()
    };
    let config = ArtificialClockConfig {
        start_at: fixed_time,
        mode: ArtificialMode::Manual,
    };
    let (clock, _ctrl) = ClockHandle::artificial(config);

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

    // The recorded_at should match the artificial clock's time
    assert_eq!(recorded_at, fixed_time);

    Ok(())
}
