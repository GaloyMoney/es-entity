// mod helpers;

// use es_entity::*;

// use std::sync::Arc;
// use tokio::sync::Mutex;

// pub type Op = Arc<Mutex<DbOp<'static>>>;

// #[tokio::test]
// async fn check() -> anyhow::Result<()> {
//     let pool = helpers::init_pool().await?;
//     accept_ex(&pool).await;
//     let mut tx = pool.begin().await?;
//     accept_op(&mut tx).await;
//     accept_op(&mut tx).await;
//     let mut op = DbOp::init(&pool).await?;
//     accept_db_op(&mut op).await?;
//     accept_db_op(&mut op).await?;
//     Ok(())
// }
// async fn accept_db_op(op: &mut DbOp<'_>) -> anyhow::Result<()> {
//     accept_op(op).await;
//     accept_op(op).await;
//     Ok(())
// }

// async fn accept_op(op: &mut impl AtomicOperation) {
//     {
//         let op = &mut *op;
//         accept_ex(op).await;
//     }
//     accept_exec(op.as_executor()).await;
//     accept_other_op(op).await;
//     accept_exec(op.as_executor()).await;
//     accept_ex(op).await;
// }
// async fn accept_other_op(op: &mut impl AtomicOperation) {
//     accept_ex(&mut *op).await;
//     accept_exec(op.as_executor()).await;
//     accept_exec(op.as_executor()).await;
//     accept_ex(op).await;
// }

// async fn accept_ex<'c>(ex: impl IntoExecutor<'c>) {
//     accept_exec(ex.into_executor()).await
// }

// async fn accept_exec(_ex: impl sqlx::Executor<'_, Database = sqlx::Postgres>) {}
