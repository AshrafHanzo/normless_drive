//! Connection pool and migrations.
//!
//! tokio-postgres rather than sqlx — see the note in Cargo.toml. The short
//! version: grammers links the native sqlite3 library unconditionally, and
//! so does sqlx's SQLite driver, and Cargo allows only one package to do so.

use deadpool_postgres::{Config as PoolConfig, Pool, Runtime};
use tokio_postgres::NoTls;

pub type Db = Pool;

/// Builds the pool from a `postgres://user:pass@host:port/dbname` URL.
///
/// deadpool wants the parts separately, and tokio_postgres already knows how
/// to parse the URL — so parse once and copy the pieces across rather than
/// writing a second, worse URL parser.
pub async fn connect(database_url: &str) -> anyhow::Result<Db> {
    let parsed: tokio_postgres::Config = database_url
        .parse()
        .map_err(|error| anyhow::anyhow!("DATABASE_URL is not a valid connection string: {error}"))?;

    let mut config = PoolConfig::new();
    config.host = parsed.get_hosts().first().map(|host| match host {
        tokio_postgres::config::Host::Tcp(name) => name.clone(),
        #[cfg(unix)]
        tokio_postgres::config::Host::Unix(path) => path.to_string_lossy().into_owned(),
    });
    config.port = parsed.get_ports().first().copied();
    config.user = parsed.get_user().map(str::to_string);
    config.password = parsed
        .get_password()
        .map(|bytes| String::from_utf8_lossy(bytes).into_owned());
    config.dbname = parsed.get_dbname().map(str::to_string);

    config.pool = Some(deadpool_postgres::PoolConfig {
        // Four cores on the target host, and every request is IO-bound on
        // either Postgres or Telegram. More connections than this just moves
        // contention into the database.
        max_size: 16,
        ..Default::default()
    });

    let pool = config
        .create_pool(Some(Runtime::Tokio1), NoTls)
        .map_err(|error| anyhow::anyhow!("could not build the connection pool: {error}"))?;

    // Fail at boot rather than on the first request, so a bad password or a
    // stopped database is obvious in the startup log.
    pool.get()
        .await
        .map_err(|error| anyhow::anyhow!("could not reach the database: {error}"))?;

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
        (
            "0003_folders_and_shares",
            include_str!("../migrations/0003_folders_and_shares.sql"),
        ),
        (
            "0004_offline_shares",
            include_str!("../migrations/0004_offline_shares.sql"),
        ),
        (
            "0005_groups",
            include_str!("../migrations/0005_groups.sql"),
        ),
    ];

    let client = pool.get().await?;
    for (name, sql) in MIGRATIONS {
        client
            .batch_execute(sql)
            .await
            .map_err(|error| anyhow::anyhow!("migration {name} failed: {error}"))?;
        tracing::debug!(migration = name, "applied");
    }

    tracing::info!(count = MIGRATIONS.len(), "schema applied");
    Ok(())
}
