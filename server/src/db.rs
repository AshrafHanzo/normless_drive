//! Connection pool and migrations.

use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub type Db = PgPool;

pub async fn connect(database_url: &str) -> anyhow::Result<Db> {
    let pool = PgPoolOptions::new()
        // Four cores on the target host, and every request is IO-bound on
        // either Postgres or Telegram. More connections than this just moves
        // contention into the database.
        .max_connections(16)
        .acquire_timeout(Duration::from_secs(10))
        .connect(database_url)
        .await?;

    Ok(pool)
}

/// Applies the schema. Every statement is `IF NOT EXISTS`, so this is safe to
/// run on every boot and needs no version table yet. Revisit once a migration
/// has to alter or drop something.
pub async fn migrate(pool: &Db) -> anyhow::Result<()> {
    let sql = include_str!("../migrations/0001_init.sql");
    sqlx::raw_sql(sql).execute(pool).await?;
    tracing::info!("schema applied");
    Ok(())
}
