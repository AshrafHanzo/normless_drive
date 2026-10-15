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
            .route("/password", web::post().to(change_password))
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

#[derive(Deserialize)]
pub struct PasswordChange {
    pub current_password: String,
    pub new_password: String,
}

/// Changes the account password, re-wrapping the stored Telegram session so
/// it stays readable.
///
/// This is the one operation that must touch `tg_sessions`. The session is
/// encrypted under a key derived from the password (see `crate::crypto`), so
/// changing the password without re-wrapping would leave a row that nothing
/// can ever decrypt again — the user's Telegram connection would be silently
/// destroyed and they would only find out the next time they used it.
async fn change_password(
    user: CurrentUser,
    body: web::Json<PasswordChange>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    validate_password(&body.new_password).map_err(ApiError::BadRequest)?;

    let stored: String = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = $1")
        .bind(user.id)
        .fetch_one(pool.get_ref())
        .await?;

    let ok = verify_password(&body.current_password, &stored)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    if !ok {
        return Err(ApiError::Forbidden("Your current password is wrong.".into()));
    }

    let new_hash = hash_password(&body.new_password)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

    // Everything below has to land together: a password updated without its
    // session re-wrapped is an unrecoverable state, so it goes in one
    // transaction that either fully applies or does not happen at all.
    let mut tx = pool.begin().await?;

    let session_row: Option<(Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>)> = sqlx::query_as(
        "SELECT session_enc, session_nonce, api_hash_enc, api_hash_nonce, kdf_salt
         FROM tg_sessions WHERE user_id = $1 FOR UPDATE",
    )
    .bind(user.id)
    .fetch_optional(&mut *tx)
    .await?;

    if let Some((session_enc, session_nonce, api_hash_enc, api_hash_nonce, salt)) = session_row {
        let old_key = crate::crypto::derive_key(&body.current_password, &salt)
            .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

        // A failure here means the stored session was already unreadable,
        // which is a fault worth surfacing rather than papering over by
        // dropping the row.
        let session = crate::crypto::decrypt(&old_key, &session_enc, &session_nonce)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("stored session did not decrypt")))?;
        let api_hash = crate::crypto::decrypt(&old_key, &api_hash_enc, &api_hash_nonce)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("stored api hash did not decrypt")))?;

        // A fresh salt as well as a fresh key: no reason to keep the old one
        // around once everything under it is being rewritten.
        let new_salt = crate::crypto::generate_salt();
        let new_key = crate::crypto::derive_key(&body.new_password, &new_salt)
            .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

        let (session_enc, session_nonce) = crate::crypto::encrypt(&new_key, &session)
            .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
        let (api_hash_enc, api_hash_nonce) = crate::crypto::encrypt(&new_key, &api_hash)
            .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

        sqlx::query(
            "UPDATE tg_sessions
             SET session_enc = $1, session_nonce = $2,
                 api_hash_enc = $3, api_hash_nonce = $4, kdf_salt = $5
             WHERE user_id = $6",
        )
        .bind(&session_enc)
        .bind(&session_nonce)
        .bind(&api_hash_enc)
        .bind(&api_hash_nonce)
        .bind(new_salt.as_slice())
        .bind(user.id)
        .execute(&mut *tx)
        .await?;
    }

    sqlx::query("UPDATE users SET password_hash = $1 WHERE id = $2")
        .bind(&new_hash)
        .bind(user.id)
        .execute(&mut *tx)
        .await?;

    // Every other login is invalidated: a password change is how someone
    // responds to a session they think is stolen, so leaving the rest alive
    // would defeat the point.
    sqlx::query("DELETE FROM app_sessions WHERE user_id = $1")
        .bind(user.id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    audit(pool.get_ref(), Some(user.id), Some(user.id), "user.password_changed", None).await;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "ok": true,
        "message": "Password changed. Please sign in again."
    })))
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
