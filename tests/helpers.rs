use sqlx::Postgres;
use sqlx::pool::PoolConnection;

pub async fn init_pool() -> anyhow::Result<sqlx::PgPool> {
    let pg_con = std::env::var("PG_CON").unwrap();
    let pool = sqlx::PgPool::connect(&pg_con).await?;
    Ok(pool)
}

/// Advisory-lock key serializing the undecodable-blob window in
/// `corrupt_snapshot_blob_is_a_hard_error` against the unscoped
/// `list_by_id` walk in `batch_load_mixes_snapshot_states`. Distinct from
/// the key used by `tests/hooks.rs`.
const SNAPSHOT_CORRUPTION_LOCK: i64 = 512_337_004_219;

/// Session-level advisory lock held on a dedicated connection.
///
/// The lock is released by [`release`](Self::release), or automatically
/// when the connection drops (test panic or process exit), since session
/// advisory locks live and die with the connection.
pub struct SnapshotCorruptionGuard(PoolConnection<Postgres>);

impl SnapshotCorruptionGuard {
    pub async fn acquire(pool: &sqlx::PgPool) -> anyhow::Result<Self> {
        let mut conn = pool.acquire().await?;
        sqlx::query("SELECT pg_advisory_lock($1)")
            .bind(SNAPSHOT_CORRUPTION_LOCK)
            .execute(&mut *conn)
            .await?;
        Ok(Self(conn))
    }

    pub async fn release(self) -> anyhow::Result<()> {
        let mut conn = self.0;
        sqlx::query("SELECT pg_advisory_unlock($1)")
            .bind(SNAPSHOT_CORRUPTION_LOCK)
            .execute(&mut *conn)
            .await?;
        Ok(())
    }
}
