// mod helpers;

// use es_entity::*;

// use sqlx::PgPool;

// trait RunJob {
//     fn execute(&self) -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>>;
// }

// struct Job {
//     pool: PgPool,
// }

// impl RunJob for Job {
//     fn execute(&self) -> std::pin::Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + '_>> {
//         let pool = self.pool.clone();
//         Box::pin(async move {
//             // let mut tx = pool.begin().await?;
//             accept_ex(&pool).await;
//             accept_ex(&pool).await;

//             accept_one_time(&pool).await;
//             accept_one_time(&pool).await;
//             let mut tx = pool.begin().await?;
//             accept_ex(tx.as_executor()).await;
//             accept_op(&mut tx).await;
//             // accept_one_time(&mut *tx).await;
//             accept_one_time(&mut tx).await;
//             accept_one_time(&mut tx).await;
//             // accept_one_time(&mut *tx).await;
//             accept_ex(tx.as_executor()).await;
//             // accept_op(&mut tx).await;
//             // accept_tx(&mut tx).await;
//             Ok(())
//         })
//     }
// }

// #[tokio::test]
// async fn execute() -> anyhow::Result<()> {
//     let pool = helpers::init_pool().await?;
//     let job = Job { pool };
//     job.execute().await?;
//     Ok(())
// }

// async fn accept_one_time(_ex: impl es_entity::IntoOneTimeExecutor<'_>) {
//     ()
// }

// async fn accept_ex(_ex: impl sqlx::Executor<'_, Database = sqlx::Postgres>) {
//     // accept_exec(ex).await
// }

// async fn accept_op<OP: es_entity::AtomicOperation>(op: &mut OP) -> anyhow::Result<()> {
//     accept_ex(op.as_executor()).await;
//     sqlx::query!("SELECT NOW()")
//         .fetch_all(op.as_executor())
//         .await?;
//     Ok(())
// }
