mod entities;
mod helpers;

use entities::{order::*, profile::*, user::*};
use es_entity::*;
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

/// Same shape as the nested repo in tests/nested_entities.rs — used here
/// standalone to violate the hand-written `order_items_order_id_fkey` foreign
/// key through generated ops.
#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "OrderItem",
    delete = "soft",
    columns(order_id(ty = "OrderId", update(persist = false), parent))
)]
pub struct OrderItems {
    pool: PgPool,
}

impl OrderItems {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

fn rejected(
    err: es_entity::RepoWriteError<ProfileConstraintViolation>,
) -> ProfileConstraintViolation {
    match err {
        Fail::Rejected(cv) => cv,
        other => panic!("expected Rejected, got {other:?}"),
    }
}

fn rejected_user(
    err: es_entity::RepoWriteError<UserConstraintViolation>,
) -> UserConstraintViolation {
    match err {
        Fail::Rejected(cv) => cv,
        other => panic!("expected Rejected, got {other:?}"),
    }
}

fn rejected_order_item(
    err: es_entity::RepoWriteError<OrderItemConstraintViolation>,
) -> OrderItemConstraintViolation {
    match err {
        Fail::Rejected(cv) => cv,
        other => panic!("expected Rejected, got {other:?}"),
    }
}

// ===========================================================================
// Constraint violation tests
// ===========================================================================

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
    let cv = rejected(err);

    assert!(
        matches!(&cv, ProfileConstraintViolation::IdxProfilesEmail(c) if c.attempted.as_ref() == Some(&email))
    );
    assert!(!cv.to_string().contains(&email));
    assert!(
        std::error::Error::source(&cv)
            .unwrap()
            .source()
            .unwrap()
            .source()
            .unwrap()
            .is::<sqlx::Error>()
    );
    assert_eq!(cv.constraint_name(), "idx_profiles_email");
    assert!(matches!(
        &cv,
        ProfileConstraintViolation::IdxProfilesEmail(_)
    ));
    assert_eq!(cv.kind(), ConstraintKind::Unique);
    assert!(cv.is_unique());
    assert!(!cv.is_foreign_key());
    assert!(!cv.is_check());

    Ok(())
}

#[tokio::test]
async fn create_duplicate_id_returns_constraint_violation_with_value() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let id = UserId::new();

    let first = NewUser::builder().id(id).name("First").build().unwrap();
    users.create(first).await?;

    let duplicate = NewUser::builder().id(id).name("Second").build().unwrap();
    let err = match users.create(duplicate).await {
        Err(e) => e,
        Ok(_) => panic!("expected constraint violation"),
    };
    let cv = rejected_user(err);

    assert!(matches!(&cv, UserConstraintViolation::Pkey(c) if c.attempted == Some(id)));
    assert_eq!(cv.constraint_name(), "users_pkey");
    assert!(matches!(&cv, UserConstraintViolation::Pkey(_)));
    assert_eq!(cv.kind(), ConstraintKind::Unique);

    Ok(())
}

/// Regression test for the #196 combined index+events write: Postgres
/// interleaves the CTE (index insert) and main statement (events insert), so
/// for an intra-batch duplicate id either the index-table pkey or the
/// events-table `(id, sequence)` pkey may fire first. Both must classify as
/// the duplicate-id rejection — never `Transient`: a brand-new entity's
/// events always start at sequence 1, so this can only be a genuine
/// duplicate id, not a race.
#[tokio::test]
async fn create_all_intra_batch_duplicate_id_classifies_as_duplicate() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    // Duplicate in different positions and batch sizes, in case the chosen
    // plan (and therefore which constraint fires first) varies with shape.
    for batch_size in [2usize, 5] {
        for dup_pos in [0usize, batch_size - 1] {
            let dup_id = UserId::new();
            let new_users: Vec<_> = (0..=batch_size)
                .map(|i| {
                    let id = if i == dup_pos || i == batch_size {
                        dup_id
                    } else {
                        UserId::new()
                    };
                    NewUser::builder()
                        .id(id)
                        .name(format!("User{i}"))
                        .build()
                        .unwrap()
                })
                .collect();

            let err = match users.create_all(new_users).await {
                Err(e) => e,
                Ok(_) => panic!("expected constraint violation"),
            };

            assert_eq!(err.lane(), Lane::Rejected, "got {err:?}");
            let cv = rejected_user(err);

            assert!(matches!(&cv, UserConstraintViolation::Pkey(c) if c.attempted.is_none()));

            // The whole batch rolls back.
            assert!(users.find_by_id(dup_id).await.is_err());
        }
    }

    Ok(())
}

/// A `create_all` batch containing an id that already exists in the database
/// must classify the same way as the intra-batch case.
#[tokio::test]
async fn create_all_preexisting_duplicate_id_classifies_as_duplicate() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let id = UserId::new();
    users
        .create(NewUser::builder().id(id).name("First").build().unwrap())
        .await?;

    let new_users = vec![
        NewUser::builder()
            .id(UserId::new())
            .name("Fresh")
            .build()
            .unwrap(),
        NewUser::builder().id(id).name("Dup").build().unwrap(),
    ];
    let err = match users.create_all(new_users).await {
        Err(e) => e,
        Ok(_) => panic!("expected constraint violation"),
    };

    assert_eq!(err.lane(), Lane::Rejected);
    let cv = rejected_user(err);

    assert!(matches!(&cv, UserConstraintViolation::Pkey(c) if c.attempted.is_none()));

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
    let cv = rejected(err);

    assert!(
        matches!(&cv, ProfileConstraintViolation::IdxProfilesEmail(c) if c.attempted.as_ref() == Some(&email_a))
    );

    Ok(())
}

// ===========================================================================
// Non-unique constraint classification tests (foreign key / check)
// ===========================================================================

#[tokio::test]
async fn create_fk_violation_returns_constraint_violation() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let order_items = OrderItems::new(pool);

    // No such order exists — violates order_items_order_id_fkey.
    let item = NewOrderItem::builder()
        .id(OrderItemId::new())
        .order_id(OrderId::new())
        .product_name("Orphan")
        .quantity(1)
        .price(1.0)
        .build()
        .unwrap();
    let err = match order_items.create(item).await {
        Err(e) => e,
        Ok(_) => panic!("expected constraint violation"),
    };
    let cv = rejected_order_item(err);

    assert_eq!(cv.constraint_name(), "order_items_order_id_fkey");
    assert!(matches!(&cv, OrderItemConstraintViolation::OrderIdFkey(_)));
    assert_eq!(cv.kind(), ConstraintKind::ForeignKey);

    assert!(cv.is_foreign_key());
    assert!(!cv.is_unique());
    assert!(!cv.is_check());

    Ok(())
}

#[tokio::test]
async fn create_all_fk_violation_returns_constraint_violation() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let order_items = OrderItems::new(pool);

    let item = NewOrderItem::builder()
        .id(OrderItemId::new())
        .order_id(OrderId::new())
        .product_name("Orphan")
        .quantity(1)
        .price(1.0)
        .build()
        .unwrap();
    let err = match order_items.create_all(vec![item]).await {
        Err(e) => e,
        Ok(_) => panic!("expected constraint violation"),
    };
    let cv = rejected_order_item(err);

    assert_eq!(cv.constraint_name(), "order_items_order_id_fkey");
    assert!(matches!(&cv, OrderItemConstraintViolation::OrderIdFkey(_)));

    Ok(())
}

#[tokio::test]
async fn create_check_violation_returns_constraint_violation() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let profiles = Profiles::new(pool);

    // Blank email violates the profiles_email_not_blank CHECK constraint.
    let profile = NewProfile::builder()
        .id(ProfileId::new())
        .name("Blank")
        .email("")
        .build()
        .unwrap();
    let err = match profiles.create(profile).await {
        Err(e) => e,
        Ok(_) => panic!("expected constraint violation"),
    };
    let cv = rejected(err);

    assert_eq!(cv.constraint_name(), "profiles_email_not_blank");
    assert!(matches!(&cv, ProfileConstraintViolation::EmailNotBlank(_)));
    assert_eq!(cv.kind(), ConstraintKind::Check);

    assert!(cv.is_check());
    assert!(!cv.is_unique());
    assert!(!cv.is_foreign_key());

    Ok(())
}

#[tokio::test]
async fn update_check_violation_returns_constraint_violation() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let profiles = Profiles::new(pool);

    let email = format!("check_{}@test.com", ProfileId::new());
    let profile = NewProfile::builder()
        .id(ProfileId::new())
        .name("Check")
        .email(&email)
        .build()
        .unwrap();
    let mut profile = profiles.create(profile).await?;

    let _ = profile.update_email(String::new());
    let err = match profiles.update(&mut profile).await {
        Err(e) => e,
        Ok(_) => panic!("expected constraint violation"),
    };
    let cv = rejected(err);

    assert_eq!(cv.constraint_name(), "profiles_email_not_blank");
    assert!(matches!(&cv, ProfileConstraintViolation::EmailNotBlank(_)));

    Ok(())
}

// ===========================================================================
// Not-found error tests — D2: missing is always Fatal(Invariant), never a
// rejection. `NotFound` (the fatal's source) carries the entity/column/value.
// ===========================================================================

#[tokio::test]
async fn find_by_id_not_found_is_fatal_invariant() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let missing_id = UserId::new();
    let err = match users.find_by_id(missing_id).await {
        Err(e) => e,
        Ok(_) => panic!("expected a Fatal(Invariant) error"),
    };

    let fatal = match err {
        Fault::Fatal(fatal) => fatal,
        other => panic!("expected Fatal, got {other:?}"),
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);

    let not_found = std::error::Error::source(&fatal)
        .and_then(|s| s.downcast_ref::<NotFound>())
        .expect("Fatal's source should be a NotFound");
    assert_eq!(not_found.column, Some("id"));
    let parsed: UserId = not_found.value.parse().expect("value parses as UserId");
    assert_eq!(parsed, missing_id);

    // maybe_find_by_id tolerates absence instead.
    assert!(users.maybe_find_by_id(missing_id).await?.is_none());

    Ok(())
}

#[tokio::test]
async fn find_by_name_not_found_is_fatal_invariant() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let missing_name = format!("nonexistent_{}", UserId::new());
    let err = match users.find_by_name(&missing_name).await {
        Err(e) => e,
        Ok(_) => panic!("expected a Fatal(Invariant) error"),
    };

    let fatal = match err {
        Fault::Fatal(fatal) => fatal,
        other => panic!("expected Fatal, got {other:?}"),
    };
    let not_found = std::error::Error::source(&fatal)
        .and_then(|s| s.downcast_ref::<NotFound>())
        .expect("Fatal's source should be a NotFound");
    assert_eq!(not_found.column, Some("name"));
    assert!(
        not_found.value.contains(&missing_name),
        "NotFound's value should contain the name: got {}",
        not_found.value
    );

    assert!(users.maybe_find_by_name(&missing_name).await?.is_none());

    Ok(())
}

#[tokio::test]
async fn unknown_database_constraint_is_fatal_with_original_source() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool.clone());
    let mut op = DbOp::init(&pool).await?;
    sqlx::query("ALTER TABLE users ADD CONSTRAINT v2_unknown_constraint CHECK (name <> 'v2-unknown-constraint-trigger') NOT VALID")
        .execute(op.as_executor()).await?;
    let new = NewUser::builder()
        .id(UserId::new())
        .name("v2-unknown-constraint-trigger")
        .build()
        .unwrap();
    let error = users
        .create_in_op(&mut op, new)
        .await
        .err()
        .expect("check fails");
    let Fail::Fatal(fatal) = error else {
        panic!("unknown constraint must be fatal")
    };
    assert_eq!(fatal.kind, FatalKind::Invariant);
    assert_eq!(fatal.context.as_deref(), Some("v2_unknown_constraint"));
    let sql = std::error::Error::source(&fatal)
        .unwrap()
        .downcast_ref::<sqlx::Error>()
        .unwrap();
    assert_eq!(
        sql.as_database_error().unwrap().constraint(),
        Some("v2_unknown_constraint")
    );
    drop(op); // Transaction rollback removes the test-only constraint.
    Ok(())
}

mod composite {
    use super::entities::profile::*;
    use es_entity::*;
    use sqlx::PgPool;
    #[derive(EsRepo)]
    #[es_repo(
        entity = "Profile",
        tbl = "v2_profiles",
        events_tbl = "v2_profile_events",
        columns(
            name(ty = "String", update(accessor = "data.name")),
            email(ty = "String")
        )
    )]
    pub struct Profiles {
        pub pool: PgPool,
    }
}

#[tokio::test]
async fn composite_partial_index_preserves_typed_key_and_batch_uncertainty() -> anyhow::Result<()> {
    let profiles = composite::Profiles {
        pool: helpers::init_pool().await?,
    };
    let email = format!("composite-{}", ProfileId::new());
    let make = |name: &str| {
        NewProfile::builder()
            .id(ProfileId::new())
            .name(name)
            .email(&email)
            .build()
            .unwrap()
    };
    // Outside the predicate, duplicate key fields are allowed.
    profiles.create(make("inactive")).await?;
    profiles.create(make("inactive")).await?;
    profiles.create(make("active")).await?;
    let e = profiles
        .create(make("active"))
        .await
        .err()
        .expect("duplicate active identity");
    let Fail::Rejected(composite::ProfileConstraintViolation::ActiveIdentity(conflict)) = e else {
        panic!("wrong constraint")
    };
    assert!(!conflict.to_string().contains(&email));
    let attempted = conflict.attempted.unwrap();
    assert_eq!(attempted.name, "active");
    assert_eq!(attempted.email, email);
    let e = profiles
        .create_all(vec![make("active"), make("other")])
        .await
        .err()
        .expect("duplicate active identity");
    assert!(
        matches!(e, Fail::Rejected(composite::ProfileConstraintViolation::ActiveIdentity(c)) if c.attempted.is_none())
    );
    Ok(())
}
