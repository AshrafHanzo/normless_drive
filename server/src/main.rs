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

    warn_if_no_admin_possible(&pool, &config).await;

    let bind_addr = config.bind_addr.clone();
    tracing::info!(%bind_addr, base_url = %config.public_base_url, "starting");

    let shared_config = web::Data::new(config);
    let shared_pool = web::Data::new(pool);

    HttpServer::new(move || {
        App::new()
            .app_data(shared_pool.clone())
            .app_data(shared_config.clone())
            // Reject oversized JSON bodies early. File uploads do not come
            // through this extractor -- they stream -- so a small cap here
            // costs nothing and stops a trivial memory-exhaustion attempt.
            .app_data(web::JsonConfig::default().limit(64 * 1024))
            .wrap(Logger::default())
            .route("/health", web::get().to(health))
            .configure(auth::routes::configure)
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

/// Says so loudly if this deployment can never produce an administrator.
///
/// With no active admin and no BOOTSTRAP_ADMIN_EMAIL set, every signup lands
/// on `pending` and there is nobody who can approve them -- the service
/// looks fine and is quietly unusable. Cheaper to notice in the boot log
/// than after onboarding someone.
async fn warn_if_no_admin_possible(pool: &db::Db, config: &Config) {
    let admins: Result<i64, _> =
        sqlx::query_scalar("SELECT count(*) FROM users WHERE role = 'admin' AND status = 'active'")
            .fetch_one(pool)
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
