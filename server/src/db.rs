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

/// Applies the schema, in order, on every boot.
///
/// Each file is written to be idempotent (`IF NOT EXISTS`, `WHERE … IS NULL`)
/// so re-running is a no-op, which is why there is no version table yet. A
/// migration that cannot be made idempotent is the signal to add one.
pub async fn migrate(pool: &Db) -> anyhow::Result<()> {
    const MIGRATIONS: &[(&str, &str)] = &[
        ("0001_init", include_str!("../migrations/0001_init.sql")),
        (
            "0002_user_kdf_salt",
            include_str!("../migrations/0002_user_kdf_salt.sql"),
        ),
    ];

    for (name, sql) in MIGRATIONS {
        sqlx::raw_sql(sql)
            .execute(pool)
            .await
            .map_err(|error| anyhow::anyhow!("migration {name} failed: {error}"))?;
        tracing::debug!(migration = name, "applied");
    }

    tracing::info!(count = MIGRATIONS.len(), "schema applied");
    Ok(())
}
