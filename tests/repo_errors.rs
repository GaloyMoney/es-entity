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

    assert!(err.was_duplicate());
    assert!(err.was_duplicate_by(ProfileColumn::Email));
    assert_eq!(err.duplicate_value(), Some(email.as_str()));
    assert_eq!(err.constraint_name(), Some("idx_profiles_email"));
    assert_eq!(
        err.violated_constraint(),
        Some(ProfileConstraint::IdxProfilesEmail)
    );
    assert_eq!(
        ProfileConstraint::IdxProfilesEmail.kind(),
        ConstraintKind::Unique
    );
    assert!(!err.was_foreign_key_violation());
    assert!(!err.was_check_violation());

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

    assert!(err.was_duplicate());
    assert!(err.was_duplicate_by(UserColumn::Id));
    assert_eq!(err.duplicate_value(), Some(id.to_string().as_str()));
    assert_eq!(err.constraint_name(), Some("users_pkey"));
    assert_eq!(err.violated_constraint(), Some(UserConstraint::Pkey));
    assert_eq!(UserConstraint::Pkey.kind(), ConstraintKind::Unique);

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

    assert!(err.was_duplicate());
    assert!(err.was_duplicate_by(ProfileColumn::Email));
    assert_eq!(err.duplicate_value(), Some(email_a.as_str()));

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

    // FK violations are classified but are not duplicates.
    assert!(!err.was_duplicate());
    assert!(err.was_foreign_key_violation());
    assert!(!err.was_check_violation());
    assert_eq!(err.constraint_name(), Some("order_items_order_id_fkey"));
    assert_eq!(
        err.violated_constraint(),
        Some(OrderItemConstraint::OrderIdFkey)
    );
    assert_eq!(
        OrderItemConstraint::OrderIdFkey.kind(),
        ConstraintKind::ForeignKey
    );
    assert_eq!(err.duplicate_value(), None);
    match &err {
        OrderItemCreateError::ConstraintViolation { column: None, .. } => {}
        other => panic!("expected ConstraintViolation without column, got: {other:?}"),
    }

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

    assert!(!err.was_duplicate());
    assert!(err.was_foreign_key_violation());
    assert_eq!(err.constraint_name(), Some("order_items_order_id_fkey"));
    assert_eq!(
        err.violated_constraint(),
        Some(OrderItemConstraint::OrderIdFkey)
    );
    match &err {
        OrderItemCreateError::ConstraintViolation { column: None, .. } => {}
        other => panic!("expected ConstraintViolation without column, got: {other:?}"),
    }

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

    assert!(!err.was_duplicate());
    assert!(err.was_check_violation());
    assert!(!err.was_foreign_key_violation());
    assert_eq!(err.constraint_name(), Some("profiles_email_not_blank"));
    assert_eq!(
        err.violated_constraint(),
        Some(ProfileConstraint::EmailNotBlank)
    );
    assert_eq!(
        ProfileConstraint::EmailNotBlank.kind(),
        ConstraintKind::Check
    );
    assert_eq!(err.duplicate_value(), None);
    match &err {
        ProfileCreateError::ConstraintViolation { column: None, .. } => {}
        other => panic!("expected ConstraintViolation without column, got: {other:?}"),
    }

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

    assert!(!err.was_duplicate());
    assert!(err.was_check_violation());
    assert_eq!(err.constraint_name(), Some("profiles_email_not_blank"));
    assert_eq!(
        err.violated_constraint(),
        Some(ProfileConstraint::EmailNotBlank)
    );
    match &err {
        ProfileModifyError::ConstraintViolation { column: None, .. } => {}
        other => panic!("expected ConstraintViolation without column, got: {other:?}"),
    }

    Ok(())
}

// ===========================================================================
// Not-found error tests
// ===========================================================================

#[tokio::test]
async fn find_by_id_not_found_has_column_and_value() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let missing_id = UserId::new();
    let err = match users.find_by_id(missing_id).await {
        Err(e) => e,
        Ok(_) => panic!("expected NotFound error"),
    };

    // Column-agnostic check
    assert!(err.was_not_found());

    // Column-specific check
    assert!(err.was_not_found_by(UserColumn::Id));
    assert!(!err.was_not_found_by(UserColumn::Name));

    // Value should use Display format and be parseable back into the ID type
    let value = err.not_found_value().expect("should have a value");
    let parsed: UserId = value
        .parse()
        .expect("not_found_value should be parseable as UserId");
    assert_eq!(parsed, missing_id);

    // Pattern matching on the variant
    match &err {
        UserFindError::NotFound {
            column: Some(UserColumn::Id),
            value,
            ..
        } => {
            let parsed: UserId = value.parse().expect("value should be parseable as UserId");
            assert_eq!(parsed, missing_id);
        }
        other => panic!("expected NotFound with column Id, got: {other:?}"),
    }

    Ok(())
}

#[tokio::test]
async fn find_by_name_not_found_has_column_and_value() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let missing_name = format!("nonexistent_{}", UserId::new());
    let err = match users.find_by_name(&missing_name).await {
        Err(e) => e,
        Ok(_) => panic!("expected NotFound error"),
    };

    assert!(err.was_not_found());
    assert!(err.was_not_found_by(UserColumn::Name));
    assert!(!err.was_not_found_by(UserColumn::Id));

    let value = err.not_found_value().expect("should have a value");
    assert!(
        value.contains(&missing_name),
        "not_found_value should contain the name: got {value}"
    );

    // Pattern matching on the variant
    match &err {
        UserFindError::NotFound {
            column: Some(UserColumn::Name),
            ..
        } => {}
        other => panic!("expected NotFound with column Name, got: {other:?}"),
    }

    Ok(())
}
