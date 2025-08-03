mod entities;
mod helpers;

use entities::user::*;
use es_entity::*;
use helpers::init_pool;
use sqlx::PgPool;

mod tbl_prefix_tests {
    use super::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        tbl_prefix = "ignore_prefix",
        entity = "User",
        columns(name(ty = "String"))
    )]
    struct UsersTblPrefix {
        pool: PgPool,
    }

    impl UsersTblPrefix {
        fn new(pool: PgPool) -> Self {
            Self { pool }
        }

        async fn query_with_args(&self, id: UserId) -> Result<User, EsRepoError> {
            es_query!(
                tbl_prefix = "ignore_prefix",
                "SELECT * FROM ignore_prefix_users WHERE id = $1",
                id as UserId
            )
            .fetch_one(self.pool())
            .await
        }

        async fn query_without_args(&self) -> Result<(Vec<User>, bool), EsRepoError> {
            es_query!(
                tbl_prefix = "ignore_prefix",
                "SELECT * FROM ignore_prefix_users"
            )
            .fetch_n(self.pool(), 2)
            .await
        }
    }

    #[tokio::test]
    async fn with_args() -> anyhow::Result<()> {
        let pool = init_pool().await?;
        let users = UsersTblPrefix::new(pool);
        let id = UserId::new();
        let new_user = NewUser::builder().id(id).name("Frank").build().unwrap();

        users.create(new_user).await?;
        let loaded_user = users.query_with_args(id).await?;
        assert_eq!(loaded_user.id, id);

        Ok(())
    }

    #[tokio::test]
    async fn without_args() -> anyhow::Result<()> {
        let pool = init_pool().await?;
        let users = UsersTblPrefix::new(pool);

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
}

mod entity_tests {
    use super::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(
        tbl = "custom_name_for_users",
        events_tbl = "custom_name_for_user_events",
        entity = "User",
        columns(name(ty = "String"))
    )]
    struct UsersEntity {
        pool: PgPool,
    }

    impl UsersEntity {
        fn new(pool: PgPool) -> Self {
            Self { pool }
        }

        async fn query_with_args(&self, id: UserId) -> Result<User, EsRepoError> {
            let mut op = self.begin_op().await?;
            es_query!(
                entity = User,
                "SELECT * FROM custom_name_for_users WHERE id = $1",
                id as UserId
            )
            .fetch_one(op.as_executor())
            .await
        }

        async fn query_without_args(&self) -> Result<(Vec<User>, bool), EsRepoError> {
            let mut op = self.begin_op().await?;
            es_query!(entity = User, "SELECT * FROM custom_name_for_users")
                .fetch_n(op.as_executor(), 2)
                .await
        }
    }

    #[tokio::test]
    async fn with_args() -> anyhow::Result<()> {
        let pool = init_pool().await?;
        let users = UsersEntity::new(pool);
        let id = UserId::new();
        let new_user = NewUser::builder().id(id).name("Frank").build().unwrap();

        users.create(new_user).await?;
        let loaded_user = users.query_with_args(id).await?;
        assert_eq!(loaded_user.id, id);

        Ok(())
    }

    #[tokio::test]
    async fn without_args() -> anyhow::Result<()> {
        let pool = init_pool().await?;
        let users = UsersEntity::new(pool);

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
}

mod no_params_tests {
    use super::*;

    #[derive(EsRepo, Debug)]
    #[es_repo(entity = "User", columns(name(ty = "String")))]
    struct UsersNoParams {
        pool: PgPool,
    }

    impl UsersNoParams {
        fn new(pool: PgPool) -> Self {
            Self { pool }
        }

        async fn query_with_args(&self, id: UserId) -> Result<User, EsRepoError> {
            es_query!("SELECT * FROM users WHERE id = $1", id as UserId)
                .fetch_one(self.pool())
                .await
        }

        async fn query_without_args(&self) -> Result<(Vec<User>, bool), EsRepoError> {
            es_query!("SELECT * FROM users")
                .fetch_n(self.pool(), 2)
                .await
        }
    }

    #[tokio::test]
    async fn with_args() -> anyhow::Result<()> {
        let pool = init_pool().await?;
        let users = UsersNoParams::new(pool);
        let id = UserId::new();
        let new_user = NewUser::builder().id(id).name("Frank").build().unwrap();

        users.create(new_user).await?;
        let loaded_user = users.query_with_args(id).await?;
        assert_eq!(loaded_user.id, id);

        Ok(())
    }

    #[tokio::test]
    async fn without_args() -> anyhow::Result<()> {
        let pool = init_pool().await?;
        let users = UsersNoParams::new(pool);

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
}
