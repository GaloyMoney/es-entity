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

fn rejected(err: Fail<ProfileConstraintViolation>) -> ProfileConstraintViolation {
    match err {
        Fail::Rejected(cv) => cv,
        other => panic!("expected Rejected, got {other:?}"),
    }
}

fn rejected_user(err: Fail<UserConstraintViolation>) -> UserConstraintViolation {
    match err {
        Fail::Rejected(cv) => cv,
        other => panic!("expected Rejected, got {other:?}"),
    }
}

fn rejected_order_item(err: Fail<OrderItemConstraintViolation>) -> OrderItemConstraintViolation {
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

    assert_eq!(cv.column(), Some(ProfileColumn::Email));
    assert_eq!(cv.value(), Some(email.as_str()));
    assert_eq!(cv.constraint_name(), Some("idx_profiles_email"));
    assert_eq!(cv.constraint(), Some(ProfileConstraint::IdxProfilesEmail));
    assert_eq!(
        ProfileConstraint::IdxProfilesEmail.kind(),
        ConstraintKind::Unique
    );
    assert!(cv.is_unique());
    assert!(!cv.is_foreign_key());
    assert!(!cv.is_check());
    assert!(cv.is_duplicate_of(ProfileColumn::Email));
    assert!(!cv.is_duplicate_of(ProfileColumn::Id));

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

    assert_eq!(cv.column(), Some(UserColumn::Id));
    assert_eq!(cv.value(), Some(id.to_string().as_str()));
    assert_eq!(cv.constraint_name(), Some("users_pkey"));
    assert_eq!(cv.constraint(), Some(UserConstraint::Pkey));
    assert_eq!(UserConstraint::Pkey.kind(), ConstraintKind::Unique);

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
            assert_eq!(cv.column(), Some(UserColumn::Id), "wrong column");
            assert_eq!(cv.value(), Some(dup_id.to_string().as_str()));

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
    assert_eq!(cv.column(), Some(UserColumn::Id));
    assert_eq!(cv.value(), Some(id.to_string().as_str()));

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

    assert_eq!(cv.column(), Some(ProfileColumn::Email));
    assert_eq!(cv.value(), Some(email_a.as_str()));

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

    assert_eq!(cv.constraint_name(), Some("order_items_order_id_fkey"));
    assert_eq!(cv.constraint(), Some(OrderItemConstraint::OrderIdFkey));
    assert_eq!(
        OrderItemConstraint::OrderIdFkey.kind(),
        ConstraintKind::ForeignKey
    );
    assert_eq!(cv.value(), None);
    assert_eq!(cv.column(), None);
    assert!(cv.is_foreign_key());
    assert!(!cv.is_unique());
    assert!(!cv.is_check());
    assert!(!cv.is_duplicate_of(OrderItemColumn::Id));

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

    assert_eq!(cv.constraint_name(), Some("order_items_order_id_fkey"));
    assert_eq!(cv.constraint(), Some(OrderItemConstraint::OrderIdFkey));
    assert_eq!(cv.column(), None);

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

    assert_eq!(cv.constraint_name(), Some("profiles_email_not_blank"));
    assert_eq!(cv.constraint(), Some(ProfileConstraint::EmailNotBlank));
    assert_eq!(
        ProfileConstraint::EmailNotBlank.kind(),
        ConstraintKind::Check
    );
    assert_eq!(cv.value(), None);
    assert_eq!(cv.column(), None);
    assert!(cv.is_check());
    assert!(!cv.is_unique());
    assert!(!cv.is_foreign_key());
    assert!(!cv.is_duplicate_of(ProfileColumn::Email));

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

    assert_eq!(cv.constraint_name(), Some("profiles_email_not_blank"));
    assert_eq!(cv.constraint(), Some(ProfileConstraint::EmailNotBlank));
    assert_eq!(cv.column(), None);

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
