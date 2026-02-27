mod entities;
mod helpers;

use entities::customer::*;
use es_entity::*;
use sqlx::PgPool;

#[derive(EsRepo, Debug)]
#[es_repo(
    entity = "Customer",
    err = "EsRepoError",
    forgettable,
    columns(email(ty = "String"))
)]
pub struct Customers {
    pool: PgPool,
}

impl Customers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn create_and_load_with_forgettable_fields() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let new_customer = NewCustomer::builder()
        .id(CustomerId::new())
        .name("Alice Smith")
        .email("alice@example.com")
        .build()
        .unwrap();

    let customer = customers.create(new_customer).await?;
    assert_eq!(customer.name, "Alice Smith");
    assert_eq!(customer.email, "alice@example.com");

    // Load the customer and verify data is intact
    let loaded = customers.find_by_id(customer.id).await?;
    assert_eq!(loaded.name, "Alice Smith");
    assert_eq!(loaded.email, "alice@example.com");

    Ok(())
}

#[tokio::test]
async fn forget_removes_forgettable_data() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Bob Jones")
        .email("bob@example.com")
        .build()
        .unwrap();

    let mut customer = customers.create(new_customer).await?;
    assert_eq!(customer.name, "Bob Jones");

    // Update the name (adds another event with a forgettable field)
    let _ = customer.update_name("Robert Jones");
    customers.update(&mut customer).await?;

    // Verify before forget
    let loaded = customers.find_by_id(id).await?;
    assert_eq!(loaded.name, "Robert Jones");
    assert_eq!(loaded.email, "bob@example.com");

    // Forget the customer's personal data - entity is updated in-place
    let mut loaded = customers.find_by_id(id).await?;
    customers.forget(&mut loaded).await?;

    assert_eq!(loaded.name, "[forgotten]");
    // Non-forgettable field should remain intact
    assert_eq!(loaded.email, "bob@example.com");

    Ok(())
}

#[tokio::test]
async fn forget_preserves_non_forgettable_events() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let id = CustomerId::new();
    let new_customer = NewCustomer::builder()
        .id(id)
        .name("Charlie")
        .email("charlie@example.com")
        .build()
        .unwrap();

    let mut customer = customers.create(new_customer).await?;

    // Update email (non-forgettable field)
    let _ = customer.update_email("charlie_new@example.com");
    customers.update(&mut customer).await?;

    // Forget and verify - entity is updated in-place
    let mut loaded = customers.find_by_id(id).await?;
    customers.forget(&mut loaded).await?;

    assert_eq!(loaded.name, "[forgotten]");
    assert_eq!(loaded.email, "charlie_new@example.com");

    Ok(())
}

#[tokio::test]
async fn find_all_works_with_forgettable() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let customers = Customers::new(pool);

    let id1 = CustomerId::new();
    let id2 = CustomerId::new();

    let c1 = NewCustomer::builder()
        .id(id1)
        .name("Dave")
        .email("dave@example.com")
        .build()
        .unwrap();
    let c2 = NewCustomer::builder()
        .id(id2)
        .name("Eve")
        .email("eve@example.com")
        .build()
        .unwrap();

    customers.create(c1).await?;
    customers.create(c2).await?;

    let all = customers.find_all::<Customer>(&[id1, id2]).await?;
    assert_eq!(all.len(), 2);
    assert_eq!(all[&id1].name, "Dave");
    assert_eq!(all[&id2].name, "Eve");

    // Forget one customer - entity is updated in-place
    let mut c1 = customers.find_by_id(id1).await?;
    customers.forget(&mut c1).await?;
    assert_eq!(c1.name, "[forgotten]");

    let all = customers.find_all::<Customer>(&[id1, id2]).await?;
    assert_eq!(all[&id1].name, "[forgotten]");
    assert_eq!(all[&id2].name, "Eve");

    Ok(())
}
