mod entities;
mod helpers;

use entities::{profile::*, user::*};
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
async fn list_for_filters() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);

    // Test with default filters (no filter) - should return all entities
    let PaginatedQueryRet {
        entities,
        has_next_page: _,
        end_cursor: _,
    } = users
        .list_for_filters(
            UsersFilters::default(),
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

    // Create a user with a unique name for testing the filter
    let unique_name = format!("FiltersTest_{}", UserId::new());
    let new_user = NewUser::builder()
        .id(UserId::new())
        .name(&unique_name)
        .build()
        .unwrap();

    users.create(new_user).await?;

    // Test with specific name filter
    let filtered_result = users
        .list_for_filters(
            UsersFilters {
                name: Some(unique_name.clone()),
            },
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

    assert_eq!(filtered_result.entities.len(), 1);
    assert_eq!(filtered_result.entities[0].name, unique_name);

    // Test pagination with filters
    let paginated_result = users
        .list_for_filters(
            UsersFilters::default(),
            Sort {
                by: UsersSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 1,
                after: None,
            },
        )
        .await?;

    assert_eq!(paginated_result.entities.len(), 1);
    assert!(paginated_result.has_next_page);

    // Use cursor for next page
    let next_page = users
        .list_for_filters(
            UsersFilters::default(),
            Sort {
                by: UsersSortBy::Id,
                direction: ListDirection::Ascending,
            },
            PaginatedQueryArgs {
                first: 1,
                after: paginated_result.end_cursor,
            },
        )
        .await?;

    assert_eq!(next_page.entities.len(), 1);
    assert_ne!(paginated_result.entities[0].id, next_page.entities[0].id);

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

#[tokio::test]
async fn update_all() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let new_users = vec![
        NewUser::builder()
            .id(UserId::new())
            .name("Alice")
            .build()
            .unwrap(),
        NewUser::builder()
            .id(UserId::new())
            .name("Bob")
            .build()
            .unwrap(),
        NewUser::builder()
            .id(UserId::new())
            .name("Charlie")
            .build()
            .unwrap(),
    ];

    let mut created = users.create_all(new_users).await?;
    assert_eq!(created.len(), 3);

    let _ = created[0].update_name("Alice_updated");
    let _ = created[1].update_name("Bob_updated");
    // Leave created[2] unchanged to test skipping

    let n_events = users.update_all(&mut created).await?;
    assert_eq!(n_events, 2);

    let loaded_alice = users.find_by_id(created[0].id).await?;
    assert_eq!(loaded_alice.name, "Alice_updated");

    let loaded_bob = users.find_by_id(created[1].id).await?;
    assert_eq!(loaded_bob.name, "Bob_updated");

    let loaded_charlie = users.find_by_id(created[2].id).await?;
    assert_eq!(loaded_charlie.name, "Charlie");

    Ok(())
}

/// Profiles repo with custom accessors:
/// - `name`: field-path accessor (`data.name`) — accesses nested struct field
/// - `display_name`: method-call accessor (`display_name()`) — returns owned String
/// - `email`: direct field access — no custom accessor
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Profile",
    columns(
        name(ty = "String", update(accessor = "data.name")),
        display_name(
            ty = "String",
            create(accessor = "display_name()"),
            update(accessor = "display_name()")
        ),
        email(ty = "String"),
    )
)]
pub struct Profiles {
    pool: PgPool,
}

impl Profiles {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn update_all_with_custom_accessors() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let profiles = Profiles::new(pool);

    let alice_email = format!("alice_{}@test.com", ProfileId::new());
    let bob_email = format!("bob_{}@test.com", ProfileId::new());
    let bob_new_email = format!("bob_new_{}@test.com", ProfileId::new());

    let new_profiles = vec![
        NewProfile::builder()
            .id(ProfileId::new())
            .name("Alice")
            .email(&alice_email)
            .build()
            .unwrap(),
        NewProfile::builder()
            .id(ProfileId::new())
            .name("Bob")
            .email(&bob_email)
            .build()
            .unwrap(),
    ];

    let mut created = profiles.create_all(new_profiles).await?;
    assert_eq!(created.len(), 2);

    let _ = created[0].update_name("Alice_updated");
    let _ = created[1].update_email(bob_new_email.clone());

    let n_events = profiles.update_all(&mut created).await?;
    assert_eq!(n_events, 2);

    let loaded_alice = profiles.find_by_id(created[0].id).await?;
    assert_eq!(loaded_alice.data.name, "Alice_updated");
    assert_eq!(loaded_alice.email, alice_email);

    let loaded_bob = profiles.find_by_id(created[1].id).await?;
    assert_eq!(loaded_bob.data.name, "Bob");
    assert_eq!(loaded_bob.email, bob_new_email);

    Ok(())
}

#[tokio::test]
async fn create_duplicate_email_returns_constraint_violation_with_value() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let profiles = Profiles::new(pool);

    let email = format!("unique_{}@test.com", ProfileId::new());

    let first = NewProfile::builder()
        .id(ProfileId::new())
        .name("First")
        .email(&email)
        .build()
        .unwrap();
    profiles.create(first).await?;

    let duplicate = NewProfile::builder()
        .id(ProfileId::new())
        .name("Second")
        .email(&email)
        .build()
        .unwrap();
    let err = match profiles.create(duplicate).await {
        Err(e) => e,
        Ok(_) => panic!("expected constraint violation"),
    };

    assert!(err.was_duplicate(ProfileColumn::Email));
    assert_eq!(err.duplicate_value(), Some(email.as_str()));

    Ok(())
}

#[tokio::test]
async fn create_duplicate_id_returns_constraint_violation_with_value() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let id = UserId::new();

    let first = NewUser::builder()
        .id(id)
        .name("First")
        .build()
        .unwrap();
    users.create(first).await?;

    let duplicate = NewUser::builder()
        .id(id)
        .name("Second")
        .build()
        .unwrap();
    let err = match users.create(duplicate).await {
        Err(e) => e,
        Ok(_) => panic!("expected constraint violation"),
    };

    assert!(err.was_duplicate(UserColumn::Id));
    assert_eq!(err.duplicate_value(), Some(id.to_string().as_str()));

    Ok(())
}

#[tokio::test]
async fn update_to_duplicate_email_returns_constraint_violation_with_value() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let profiles = Profiles::new(pool);

    let email_a = format!("update_a_{}@test.com", ProfileId::new());
    let email_b = format!("update_b_{}@test.com", ProfileId::new());

    let profile_a = NewProfile::builder()
        .id(ProfileId::new())
        .name("A")
        .email(&email_a)
        .build()
        .unwrap();
    profiles.create(profile_a).await?;

    let profile_b = NewProfile::builder()
        .id(ProfileId::new())
        .name("B")
        .email(&email_b)
        .build()
        .unwrap();
    let mut b = profiles.create(profile_b).await?;

    // Update B's email to A's email — should trigger constraint violation
    let _ = b.update_email(email_a.clone());
    let err = match profiles.update(&mut b).await {
        Err(e) => e,
        Ok(_) => panic!("expected constraint violation"),
    };

    assert!(err.was_duplicate(ProfileColumn::Email));
    assert_eq!(err.duplicate_value(), Some(email_a.as_str()));

    Ok(())
}
