mod helpers;
mod user;

use es_entity::*;
use sqlx::PgPool;

use user::*;

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
async fn create() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);
    let id = UserId::new();
    let new_user = NewUser::builder().id(id).name("Frank").build().unwrap();
    let _ = users.create(new_user).await?;

    let loaded_user = users.find_by_id(id).await?;
    assert_eq!(loaded_user.id, id);

    Ok(())
}
