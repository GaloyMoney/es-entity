use crate::{helpers::init_pool, user::*};
use es_entity::*;
use sqlx::PgPool;

#[derive(EsRepo, Debug)]
#[es_repo(
    tbl_prefix = "ignore_prefix",
    entity = "User",
    columns(name(ty = "String"))
)]
pub struct Users {
    pool: PgPool,
}

impl Users {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn query_with_args(&self, id: UserId) -> Result<User, EsRepoError> {
        es_query!(
            [tbl_prefix = "ignore_prefix", db = self.pool()],
            "SELECT * FROM ignore_prefix_users WHERE id = $1",
            id as UserId
        )
        .fetch_one()
        .await
    }

    pub async fn query_without_args(&self) -> Result<(Vec<User>, bool), EsRepoError> {
        es_query!(
            [tbl_prefix = "ignore_prefix", db = self.pool()],
            "SELECT * FROM ignore_prefix_users"
        )
        .fetch_n(2)
        .await
    }
}

#[tokio::test]
async fn with_args() -> anyhow::Result<()> {
    let pool = init_pool().await?;

    let users = Users::new(pool);
    let id = UserId::new();
    let new_user = NewUser::builder().id(id).name("Frank").build().unwrap();
    let _ = users.create(new_user).await?;

    let loaded_user = users.query_with_args(id).await?;
    assert_eq!(loaded_user.id, id);

    Ok(())
}

#[tokio::test]
async fn without_args() -> anyhow::Result<()> {
    let pool = init_pool().await?;
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
