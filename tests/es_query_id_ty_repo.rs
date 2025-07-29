mod helpers;
mod user_with_id_ty;

use es_entity::*;
use sqlx::PgPool;
use uuid::Uuid;

use user_with_id_ty::*;

#[derive(EsRepo, Debug)]
#[es_repo(
    id = Uuid,
    entity = "User",
    err = "EsRepoError",
    columns(name(ty = "String"))
)]
pub struct Users3 {
    pool: PgPool,
}

impl Users3 {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
    pub async fn query_with_args(&self, id: Uuid) -> Result<User, EsRepoError> {
        es_query!(
            id_ty = Uuid,
            self.pool(),
            "SELECT * FROM users WHERE id = $1",
            id
        )
        .fetch_one()
        .await
    }

    pub async fn query_without_args(&self) -> Result<(Vec<User>, bool), EsRepoError> {
        es_query!(id_ty = Uuid, self.pool(), "SELECT * FROM users")
            .fetch_n(2)
            .await
    }
}

#[tokio::test]
async fn test_query_with_args() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users3::new(pool);
    let id = Uuid::new_v4();
    let new_user = NewUser::builder().id(id).name("Frank").build().unwrap();
    let _ = users.create(new_user).await?;

    let loaded_user = users.query_with_args(id).await?;
    assert_eq!(loaded_user.id, id);

    Ok(())
}

#[tokio::test]
async fn test_query_without_args() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users3::new(pool);

    let user1 = NewUser::builder()
        .id(Uuid::new_v4())
        .name("Alice")
        .build()
        .unwrap();
    let user2 = NewUser::builder()
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
