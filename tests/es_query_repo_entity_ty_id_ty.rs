mod customer_with_id_ty;
mod helpers;

use es_entity::*;
use sqlx::PgPool;
use uuid::Uuid;

use customer_with_id_ty::*;
// crud on customer entities(not using CustomerId) stored in users
#[derive(EsRepo, Debug)]
#[es_repo(
    id = Uuid,
    tbl = "users",
    events_tbl = "user_events",
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
    pub async fn query_with_args(&self, id: Uuid) -> Result<Customer, EsRepoError> {
        es_query!(
            entity_ty = Customer,
            id_ty = Uuid,
            self.pool(),
            "SELECT * FROM users WHERE id = $1",
            id
        )
        .fetch_one()
        .await
    }

    pub async fn query_without_args(&self) -> Result<(Vec<Customer>, bool), EsRepoError> {
        es_query!(
            entity_ty = Customer,
            id_ty = Uuid,
            self.pool(),
            "SELECT * FROM users"
        )
        .fetch_n(2)
        .await
    }
}

#[tokio::test]
async fn test_es_query_with_entity_ty_and_id_ty_and_args() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Customers::new(pool);
    let id = Uuid::new_v4();
    let new_user = NewCustomer::builder().id(id).name("Frank").build().unwrap();
    let _ = users.create(new_user).await?;

    let loaded_user = users.query_with_args(id).await?;
    assert_eq!(loaded_user.id, id);

    Ok(())
}

#[tokio::test]
async fn test_es_query_with_entity_ty_and_id_ty() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Customers::new(pool);

    let user1 = NewCustomer::builder()
        .id(Uuid::new_v4())
        .name("Alice")
        .build()
        .unwrap();
    let user2 = NewCustomer::builder()
        .id(Uuid::new_v4())
        .name("Bob")
        .build()
        .unwrap();

    users.create(user1).await?;
    users.create(user2).await?;

    let (users_list, _) = users.query_without_args().await?;

    assert_eq!(users_list.len(), 2);

    Ok(())
}
