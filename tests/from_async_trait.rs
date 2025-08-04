mod entities;
mod helpers;

use entities::user::*;
use es_entity::*;
use helpers::init_pool;
use sqlx::PgPool;

trait RunJob {
    fn execute(&self) -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
}

struct TestJob {
    pool: PgPool,
}

impl RunJob for TestJob {
    fn execute(&self) -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
        let pool = self.pool.clone();
        Box::pin(async move {
            let users = Users::new(pool);
            let new_user = NewUser::builder()
                .id(UserId::new())
                .name("Frank")
                .build()
                .unwrap();

            let mut user = users.create(new_user).await?;
            let mut op = users.begin_op().await?;

            if user.update_name("Dweezil").did_execute() {
                users.update_in_op(&mut op, &mut user).await?;
            }

            Ok(())
        })
    }
}

#[derive(EsRepo, Debug)]
#[es_repo(entity = "User", columns(name(ty = "String")))]
struct Users {
    pool: PgPool,
}

impl Users {
    fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

#[tokio::test]
async fn executo() -> anyhow::Result<()> {
    let pool = init_pool().await?;

    TestJob { pool }.execute().await?;

    Ok(())
}
