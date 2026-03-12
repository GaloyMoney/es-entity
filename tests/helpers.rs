#[cfg(feature = "postgres")]
pub async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
    let pg_host = std::env::var("PG_HOST").unwrap_or("localhost".to_string());
    let pg_con = format!("postgres://user:password@{pg_host}:5432/pg");
    let pool = sqlx::PgPool::connect(&pg_con).await?;
    Ok(pool)
}

#[cfg(feature = "sqlite")]
pub async fn init_pool() -> anyhow::Result<sqlx::SqlitePool> {
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let db_id = COUNTER.fetch_add(1, Ordering::Relaxed);
    let url = format!("sqlite:file:memdb_{db_id}?mode=memory&cache=shared");
    let pool = sqlx::SqlitePool::connect(&url).await?;
    sqlx::migrate!("./migrations-sqlite").run(&pool).await?;
    Ok(pool)
}
