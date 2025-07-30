mod helpers;
mod user;

use es_entity::*;
use sqlx::PgPool;

use user::*;
// crud on User entities stored in users
#[derive(EsRepo, Debug)]
#[es_repo(entity = "User", err = "EsRepoError", columns(name(ty = "String")))]
pub struct Users {
    pool: PgPool,
}

impl Users {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn query_with_args(&self, id: UserId) -> Result<User, EsRepoError> {
        es_query!(
            self.pool(),
            "SELECT * FROM users WHERE id = $1",
            id as UserId
        )
        .fetch_one()
        .await
    }

    pub async fn query_without_args(&self) -> Result<(Vec<User>, bool), EsRepoError> {
        es_query!(self.pool(), "SELECT * FROM users")
            .fetch_n(2)
            .await
    }
}

#[tokio::test]
async fn test_es_query_with_args() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);
    let id = UserId::new();
    let new_user = NewUser::builder().id(id).name("Frank").build().unwrap();
    let _ = users.create(new_user).await?;

    let loaded_user = users.query_with_args(id).await?;
    assert_eq!(loaded_user.id, id);

    Ok(())
}

#[tokio::test]
async fn test_es_query() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;
    let users = Users::new(pool);

    let user1 = NewUser::builder()
        .id(UserId::new())
        .name("Alice")
        .build()
        .unwrap();
    let user2 = NewUser::builder()
        .id(UserId::new())
        .name("Bob")
        .build()
        .unwrap();

    users.create(user1).await?;
    users.create(user2).await?;

    let (users_list, _) = users.query_without_args().await?;

    assert_eq!(users_list.len(), 2);

    Ok(())
}
