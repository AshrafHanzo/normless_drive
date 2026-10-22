//! Normless Drive — browser front end for Telegram-backed storage.
//!
//! See docs/ARCHITECTURE.md. In particular section 2 explains why Telegram
//! sessions are encrypted with a key derived from each user's password, and
//! what that design does and does not protect against.

mod admin;
mod auth;
mod config;
mod crypto;
mod db;
mod error;
mod keycache;
mod permissions;
mod shares;

mod telegram;

use actix_web::{middleware::Logger, web, App, HttpResponse, HttpServer};
use config::Config;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,sqlx=warn".into()),
        )
        .init();

    let config = Config::from_env().map_err(|error| {
        // Config problems are the most common deployment failure, so say
        // exactly which variable rather than dumping a Debug struct.
        anyhow::anyhow!("configuration error: {error}")
    })?;

    let pool = db::connect(&config.database_url).await?;
    db::migrate(&pool).await?;

    seed_bootstrap_admin(&pool, &config).await?;
    warn_if_no_admin_possible(&pool, &config).await;

    let bind_addr = config.bind_addr.clone();
    tracing::info!(%bind_addr, base_url = %config.public_base_url, "starting");

    let shared_config = web::Data::new(config);
    let shared_pool = web::Data::new(pool);
    let shared_keys = web::Data::new(keycache::KeyCache::new());
    let shared_pending = web::Data::new(std::sync::Arc::new(telegram::PendingLogins::new()));
    let shared_clients = web::Data::new(std::sync::Arc::new(telegram::connect::Clients::new()));

    // Idle keys would otherwise sit in memory until their cookie expired,
    // two weeks later. Hourly is far more often than needed and costs
    // nothing — the map holds one entry per signed-in session.
    let sweeper = shared_keys.clone();
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let dropped = sweeper.sweep();
            if dropped > 0 {
                tracing::debug!(dropped, remaining = sweeper.len(), "swept idle keys");
            }
        }
    });

    HttpServer::new(move || {
        App::new()
            .app_data(shared_pool.clone())
            .app_data(shared_config.clone())
            .app_data(shared_keys.clone())
            .app_data(shared_pending.clone())
            .app_data(shared_clients.clone())
            // Reject oversized JSON bodies early. File uploads do not come
            // through this extractor -- they stream -- so a small cap here
            // costs nothing and stops a trivial memory-exhaustion attempt.
            .app_data(web::JsonConfig::default().limit(64 * 1024))
            .wrap(Logger::default())
            .route("/health", web::get().to(health))
            .configure(auth::routes::configure)
            .configure(telegram::routes::configure)
            .configure(telegram::files::configure)
            .configure(telegram::folders::configure)
            .configure(shares::routes::configure)
            .configure(shares::public::configure)
            .configure(admin::configure)
    })
    .bind(&bind_addr)?
    .run()
    .await?;

    Ok(())
}

/// Unauthenticated liveness probe. Deliberately says nothing about the
/// database or the build -- it is reachable from the public internet
/// through nginx.
async fn health() -> HttpResponse {
    HttpResponse::Ok().json(serde_json::json!({ "status": "ok" }))
}

/// Creates the administrator account from the environment, once.
///
/// Only ever inserts. If the address already exists this does nothing and
/// says so — a password left in the environment must not quietly reset the
/// account on every restart, which would undo any password change made
/// afterwards and would mean the plaintext in `.env` stays live forever.
async fn seed_bootstrap_admin(pool: &db::Db, config: &Config) -> anyhow::Result<()> {
    let (Some(email), Some(password)) = (
        config.bootstrap_admin_email.as_deref(),
        config.bootstrap_admin_password.as_deref(),
    ) else {
        return Ok(());
    };

    if let Err(reason) = auth::validate_password(password) {
        anyhow::bail!("BOOTSTRAP_ADMIN_PASSWORD is not acceptable: {reason}");
    }

    let client = pool.get().await?;
    let existing = client
        .query_opt(
            "SELECT id, role, status FROM users WHERE lower(email) = lower($1)",
            &[&email],
        )
        .await?;

    if let Some(row) = existing {
        let id: uuid::Uuid = row.get("id");
        let role: String = row.get("role");
        let status: String = row.get("status");

        // The account is already there. Make sure it is usable — an admin
        // who suspended themselves by accident, or who existed as a normal
        // user before being named in the environment, should be repaired
        // rather than left locked out. The password is untouched.
        if role != "admin" || status != "active" {
            client
                .execute(
                    "UPDATE users SET role = 'admin', status = 'active' WHERE id = $1",
                    &[&id],
                )
                .await?;
            tracing::warn!(%email, "restored the configured administrator to active admin");
        }
        tracing::info!(
            "administrator already exists -- remove BOOTSTRAP_ADMIN_PASSWORD from the \
             environment, it is no longer used and is plaintext on disk"
        );
        return Ok(());
    }

    let hash = auth::hash_password(password)
        .map_err(|error| anyhow::anyhow!("could not hash the bootstrap password: {error}"))?;

    // Needs a kdf_salt like any other account: the wrapping key is derived
    // from it at every login, and a NULL here would fail at sign-in rather
    // than here, which is a much worse place to find out.
    let kdf_salt = crypto::generate_salt();
    client
        .execute(
            "INSERT INTO users (email, password_hash, role, status, kdf_salt)
             VALUES ($1, $2, 'admin', 'active', $3)",
            &[&email, &hash, &kdf_salt.as_slice()],
        )
        .await?;

    tracing::warn!(
        %email,
        "created the administrator account. Sign in, change this password, then \
         remove BOOTSTRAP_ADMIN_PASSWORD from the environment."
    );

    Ok(())
}

/// Says so loudly if this deployment can never produce an administrator.
///
/// With no active admin and no BOOTSTRAP_ADMIN_EMAIL set, every signup lands
/// on `pending` and there is nobody who can approve them -- the service
/// looks fine and is quietly unusable. Cheaper to notice in the boot log
/// than after onboarding someone.
async fn warn_if_no_admin_possible(pool: &db::Db, config: &Config) {
    let admins = async {
        let client = pool.get().await?;
        let row = client
            .query_one(
                "SELECT count(*) FROM users WHERE role = 'admin' AND status = 'active'",
                &[],
            )
            .await?;
        Ok::<i64, anyhow::Error>(row.get(0))
    }
    .await;

    match admins {
        Ok(0) if config.bootstrap_admin_email.is_none() => {
            tracing::warn!(
                "no active administrator exists and BOOTSTRAP_ADMIN_EMAIL is not set -- \
                 every signup will sit on 'pending' with nobody able to approve it. \
                 Set BOOTSTRAP_ADMIN_EMAIL and register that address."
            );
        }
        Ok(0) => tracing::info!(
            admin = %config.bootstrap_admin_email.as_deref().unwrap_or("?"),
            "no administrator yet -- register this address to claim it"
        ),
        Ok(count) => tracing::info!(count, "active administrators"),
        Err(error) => tracing::warn!(?error, "could not count administrators"),
    }
}
