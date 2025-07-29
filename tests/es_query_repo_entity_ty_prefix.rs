mod customer;
mod helpers;

use es_entity::*;
use sqlx::PgPool;

use customer::*;
// crud on customer entities stored in test_users
#[derive(EsRepo, Debug)]
#[es_repo(
    tbl = "test_users",
    events_tbl = "test_user_events",
    entity = "Customer",
    err = "EsRepoError",
    columns(name(ty = "String"))
)]
pub struct Customers {
    pool: PgPool,
}

impl Customers {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn query_with_args(&self, id: CustomerId) -> Result<Customer, EsRepoError> {
        es_query!(
            entity_ty = Customer,
            "test",
            self.pool(),
            "SELECT * FROM test_users WHERE id = $1",
            id as CustomerId
        )
        .fetch_one()
        .await
    }

    pub async fn query_without_args(&self) -> Result<(Vec<Customer>, bool), EsRepoError> {
        es_query!(
            entity_ty = Customer,
            "test",
            self.pool(),
            "SELECT * FROM test_users"
        )
        .fetch_n(2)
        .await
    }
}

#[tokio::test]
async fn test_es_query_with_entity_ty_and_prefix_and_args() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Customers::new(pool);
    let id = CustomerId::new();
    let new_user = NewCustomer::builder().id(id).name("Frank").build().unwrap();
    let _ = users.create(new_user).await?;

    let loaded_user = users.query_with_args(id).await?;
    assert_eq!(loaded_user.id, id);

    Ok(())
}

#[tokio::test]
async fn test_es_query_with_entity_ty_query_and_prefix() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Customers::new(pool);

    let user1 = NewCustomer::builder()
        .id(CustomerId::new())
        .name("Alice")
        .build()
        .unwrap();
    let user2 = NewCustomer::builder()
        .id(CustomerId::new())
        .name("Bob")
        .build()
        .unwrap();

    users.create(user1).await?;
    users.create(user2).await?;

    let (users_list, _) = users.query_without_args().await?;

    assert_eq!(users_list.len(), 2);

    Ok(())
}
