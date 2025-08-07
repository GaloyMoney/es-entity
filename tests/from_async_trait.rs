mod entities;
mod helpers;

use entities::user::*;
use es_entity::*;
use helpers::init_pool;
use sqlx::PgPool;

// This test is mainly here to check if the library compiles and can be used from within
// async_trait fns.
// When initially writing the AtomicOperation / OneTimeExecutor stuff it was very painful
// to find a combination of generics that made the compiler happy and were ergonomic to use.

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
            let name = uuid::Uuid::now_v7().to_string();

            let users = Users::new(pool.clone());
            let new_user = NewUser::builder()
                .id(UserId::new())
                .name(name)
                .build()
                .unwrap();

            let mut user = users.create(new_user).await?;
            let mut op = users.begin_op().await?;

            let new_name = uuid::Uuid::now_v7().to_string();
            if user.update_name(new_name.clone()).did_execute() {
                users.update_in_op(&mut op, &mut user).await?;
            }
            let user = users.maybe_find_by_name_in_op(&pool, &*new_name).await?;
            assert!(user.is_none());

            let user = users.maybe_find_by_name_in_op(&mut op, &*new_name).await?;
            assert!(user.is_some());

            op.commit().await?;

            let user = users.maybe_find_by_name(new_name).await?;
            assert!(user.is_some());

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
async fn execute() -> anyhow::Result<()> {
    let pool = init_pool().await?;

    TestJob { pool }.execute().await?;

    Ok(())
}
