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
}

#[tokio::test]
async fn create() -> anyhow::Result<()> {
    let pool = helpers::init_pool().await?;

    let users = Users::new(pool);

    let new_user = NewUser::builder()
        .id(UserId::new())
        .name("Frank")
        .build()
        .unwrap();

    let mut user = users.create(new_user).await?;

    if user.update_name("Dweezil").did_execute() {
        users.update(&mut user).await?;
    }

    let loaded_user = users.find_by_id(user.id).await?;

    assert_eq!(user.name, loaded_user.name);

    Ok(())
}
