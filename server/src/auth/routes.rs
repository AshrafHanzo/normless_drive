//! Register, log in, log out, and "who am I".

use actix_web::{
    cookie::{Cookie, SameSite},
    web, HttpRequest, HttpResponse,
};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    generate_session_token, hash_password, hash_session_token, normalize_email, validate_password,
    verify_password, CurrentUser, Role, Status, SESSION_COOKIE,
};
use crate::{
    config::Config,
    db::Db,
    error::{ApiError, ApiResult},
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/auth")
            .route("/register", web::post().to(register))
            .route("/login", web::post().to(login))
            .route("/logout", web::post().to(logout))
            .route("/me", web::get().to(me)),
    );
}

#[derive(Deserialize)]
pub struct Credentials {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct UserView {
    pub id: Uuid,
    pub email: String,
    pub role: Role,
    pub status: Status,
}

#[derive(Serialize)]
struct RegisterResponse {
    user: UserView,
    /// Spelled out because a pending account cannot log in yet, and a signup
    /// form that just says "success" leaves people retrying the login.
    message: &'static str,
}

async fn register(
    body: web::Json<Credentials>,
    pool: web::Data<Db>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    let email = normalize_email(&body.email).map_err(ApiError::BadRequest)?;
    validate_password(&body.password).map_err(ApiError::BadRequest)?;

    let password_hash = hash_password(&body.password)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

    // Admin is granted by configured address, never by arrival order. The
    // hostname is in public DNS, so "first to register wins" would mean a
    // stranger who found the site before its owner would own it.
    let (role, status) = if config.is_bootstrap_admin(&email) {
        (Role::Admin, Status::Active)
    } else {
        (Role::User, Status::Pending)
    };

    let row: Result<(Uuid,), sqlx::Error> = sqlx::query_as(
        "INSERT INTO users (email, password_hash, role, status)
         VALUES ($1, $2, $3, $4)
         RETURNING id",
    )
    .bind(&email)
    .bind(&password_hash)
    .bind(role.as_str())
    .bind(status.as_str())
    .fetch_one(pool.get_ref())
    .await;

    let id = match row {
        Ok((id,)) => id,
        // 23505 = unique_violation on users_email_lower_key.
        Err(sqlx::Error::Database(error)) if error.code().as_deref() == Some("23505") => {
            return Err(ApiError::Conflict(
                "An account with that email already exists.".into(),
            ))
        }
        Err(other) => return Err(other.into()),
    };

    audit(pool.get_ref(), Some(id), Some(id), "user.register", None).await;

    Ok(HttpResponse::Created().json(RegisterResponse {
        user: UserView {
            id,
            email,
            role,
            status,
        },
        message: if status.may_sign_in() {
            "Account created. You are the configured administrator and can sign in now."
        } else {
            "Account created. An administrator must approve it before you can sign in."
        },
    }))
}

async fn login(
    request: HttpRequest,
    body: web::Json<Credentials>,
    pool: web::Data<Db>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    let email = normalize_email(&body.email).map_err(ApiError::BadRequest)?;

    let found: Option<(Uuid, String, String, String)> = sqlx::query_as(
        "SELECT id, password_hash, role, status FROM users WHERE lower(email) = lower($1)",
    )
    .bind(&email)
    .fetch_optional(pool.get_ref())
    .await?;

    // One message for "no such account" and for "wrong password", so the
    // endpoint cannot be used to enumerate who has an account here.
    let invalid = || ApiError::Forbidden("Incorrect email or password.".into());

    let (id, password_hash, role, status) = found.ok_or_else(invalid)?;

    let password_ok = verify_password(&body.password, &password_hash)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    if !password_ok {
        audit(pool.get_ref(), Some(id), Some(id), "user.login_failed", None).await;
        return Err(invalid());
    }

    let status = Status::parse(&status)
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("unknown status {status:?}")))?;
    if !status.may_sign_in() {
        // Distinct from a bad password: the credentials were right, so
        // saying "wrong password" would just make them reset it pointlessly.
        return Err(ApiError::Forbidden(match status {
            Status::Pending => "Your account is waiting for administrator approval.".into(),
            Status::Suspended => "Your account has been suspended.".into(),
            Status::Active => unreachable!("may_sign_in() was false"),
        }));
    }

    let role = Role::parse(&role)
        .ok_or_else(|| ApiError::Internal(anyhow::anyhow!("unknown role {role:?}")))?;

    let token = generate_session_token();
    let expires_at: DateTime<Utc> = Utc::now() + Duration::hours(config.session_ttl_hours);

    sqlx::query(
        "INSERT INTO app_sessions (user_id, token_hash, expires_at, user_agent)
         VALUES ($1, $2, $3, $4)",
    )
    .bind(id)
    .bind(&token.hash)
    .bind(expires_at)
    .bind(header(&request, "user-agent"))
    .execute(pool.get_ref())
    .await?;

    sqlx::query("UPDATE users SET last_login_at = now() WHERE id = $1")
        .bind(id)
        .execute(pool.get_ref())
        .await?;

    audit(pool.get_ref(), Some(id), Some(id), "user.login", None).await;

    Ok(HttpResponse::Ok()
        .cookie(session_cookie(&token.raw, &config, config.session_ttl_hours))
        .json(UserView {
            id,
            email,
            role,
            status,
        }))
}

async fn logout(
    request: HttpRequest,
    pool: web::Data<Db>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    if let Some(cookie) = request.cookie(SESSION_COOKIE) {
        let hash = hash_session_token(cookie.value());
        sqlx::query("DELETE FROM app_sessions WHERE token_hash = $1")
            .bind(&hash)
            .execute(pool.get_ref())
            .await?;
    }

    // Expire the cookie regardless, so a stale value cannot linger in the
    // browser even if it matched no row.
    Ok(HttpResponse::Ok()
        .cookie(session_cookie("", &config, -1))
        .json(serde_json::json!({ "ok": true })))
}

async fn me(user: CurrentUser, pool: web::Data<Db>) -> ApiResult<HttpResponse> {
    let (email, role, status): (String, String, String) =
        sqlx::query_as("SELECT email, role, status FROM users WHERE id = $1")
            .bind(user.id)
            .fetch_one(pool.get_ref())
            .await?;

    Ok(HttpResponse::Ok().json(UserView {
        id: user.id,
        email,
        role: Role::parse(&role).unwrap_or(Role::User),
        status: Status::parse(&status).unwrap_or(Status::Pending),
    }))
}

// ------------------------------------------------------------------ helpers

fn session_cookie<'a>(value: &str, config: &Config, ttl_hours: i64) -> Cookie<'a> {
    Cookie::build(SESSION_COOKIE, value.to_owned())
        .path("/")
        // Not readable from JavaScript: an XSS bug should not hand over the
        // session as well.
        .http_only(true)
        .secure(config.cookie_secure)
        // Lax rather than Strict so following a share link from elsewhere
        // still arrives signed in; the session cookie is never the sole
        // authority for a state-changing request.
        .same_site(SameSite::Lax)
        .max_age(actix_web::cookie::time::Duration::hours(ttl_hours))
        .finish()
}

fn header(request: &HttpRequest, name: &str) -> Option<String> {
    request
        .headers()
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(400).collect())
}

/// Best-effort audit write. A failure here must not fail the request that
/// triggered it — losing a log line is better than refusing a valid login.
pub async fn audit(
    pool: &Db,
    user_id: Option<Uuid>,
    actor_id: Option<Uuid>,
    action: &str,
    detail: Option<serde_json::Value>,
) {
    let result = sqlx::query(
        "INSERT INTO audit (user_id, actor_id, action, detail) VALUES ($1, $2, $3, $4)",
    )
    .bind(user_id)
    .bind(actor_id)
    .bind(action)
    .bind(detail)
    .execute(pool)
    .await;

    if let Err(error) = result {
        tracing::warn!(?error, action, "could not write audit row");
    }
}
