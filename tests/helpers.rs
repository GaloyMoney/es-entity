pub async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
    let pg_con = match std::env::var("DATABASE_URL") {
        Ok(database_url) => database_url,
        Err(_) => {
            let pg_host = std::env::var("PG_HOST").unwrap_or("localhost".to_string());
            format!("postgres://user:password@{pg_host}:5432/pg")
        }
    };
    let pool = sqlx::PgPool::connect(&pg_con).await?;
    Ok(pool)
}
