#[cfg(feature = "postgres")]
pub use postgres::*;
#[cfg(feature = "sqlite")]
pub use sqlite::*;

#[cfg(feature = "postgres")]
mod postgres {
    use sqlx::{Postgres, pool::PoolConnection};

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
}

#[cfg(feature = "sqlite")]
mod sqlite {
    use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};

    use std::{str::FromStr, time::Duration};

    /// A fresh SQLite database per call: a file in the temp dir (an
    /// in-memory database would not be shared across the pool's
    /// connections), migrated from `migrations-sqlite/`.
    pub async fn init_pool() -> anyhow::Result<es_entity::db::Pool> {
        let path =
            std::env::temp_dir().join(format!("es-entity-test-{}.sqlite", uuid::Uuid::now_v7()));
        let options = SqliteConnectOptions::from_str(&format!("sqlite://{}", path.display()))?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true)
            .busy_timeout(Duration::from_secs(30));
        let pool = SqlitePoolOptions::new()
            .max_connections(8)
            .connect_with(options)
            .await?;
        sqlx::migrate!("./migrations-sqlite").run(&pool).await?;
        Ok(pool)
    }

    /// Every test gets its own database, so there is nothing to serialize.
    pub struct SnapshotCorruptionGuard;

    impl SnapshotCorruptionGuard {
        pub async fn acquire(_pool: &es_entity::db::Pool) -> anyhow::Result<Self> {
            Ok(Self)
        }

        pub async fn release(self) -> anyhow::Result<()> {
            Ok(())
        }
    }
}
