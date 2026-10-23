//! Register, log in, log out, change password, and "who am I".

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
    crypto,
    db::Db,
    error::{is_unique_violation, ApiError, ApiResult},
    keycache::KeyCache,
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/auth")
            .route("/register", web::post().to(register))
            .route("/login", web::post().to(login))
            .route("/logout", web::post().to(logout))
            .route("/password", web::post().to(change_password))
            .route("/unlock", web::post().to(unlock))
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

    // The salt for the wrapping key is fixed at registration and only ever
    // changes when the password does, with everything under it rewritten in
    // the same transaction.
    let kdf_salt = crypto::generate_salt();

    let client = pool.get().await?;
    let inserted = client
        .query_one(
            "INSERT INTO users (email, password_hash, role, status, kdf_salt)
             VALUES ($1, $2, $3, $4, $5)
             RETURNING id",
            &[
                &email,
                &password_hash,
                &role.as_str(),
                &status.as_str(),
                &kdf_salt.as_slice(),
            ],
        )
        .await;

    let id: Uuid = match inserted {
        Ok(row) => row.get("id"),
        Err(error) if is_unique_violation(&error) => {
            return Err(ApiError::Conflict(
                "An account with that email already exists.".into(),
            ))
        }
        Err(other) => return Err(other.into()),
    };

    audit(&pool, Some(id), Some(id), "user.register", None).await;

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
    keys: web::Data<KeyCache>,
) -> ApiResult<HttpResponse> {
    let email = normalize_email(&body.email).map_err(ApiError::BadRequest)?;

    let client = pool.get().await?;
    let found = client
        .query_opt(
            "SELECT id, password_hash, role, status, kdf_salt
               FROM users WHERE lower(email) = lower($1)",
            &[&email],
        )
        .await?;

    // One message for "no such account" and for "wrong password", so the
    // endpoint cannot be used to enumerate who has an account here.
    let invalid = || ApiError::Forbidden("Incorrect email or password.".into());

    let row = found.ok_or_else(invalid)?;
    let id: Uuid = row.get("id");
    let password_hash: String = row.get("password_hash");
    let role: String = row.get("role");
    let status: String = row.get("status");
    let kdf_salt: Vec<u8> = row.get("kdf_salt");

    let password_ok = verify_password(&body.password, &password_hash)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    if !password_ok {
        audit(&pool, Some(id), Some(id), "user.login_failed", None).await;
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

    client
        .execute(
            "INSERT INTO app_sessions (user_id, token_hash, expires_at, user_agent)
             VALUES ($1, $2, $3, $4)",
            &[&id, &token.hash, &expires_at, &header(&request, "user-agent")],
        )
        .await?;

    client
        .execute("UPDATE users SET last_login_at = now() WHERE id = $1", &[&id])
        .await?;

    // This is the only point in the lifetime of a session where the
    // plaintext password exists on the server. Derive the wrapping key now
    // or the user's Telegram session stays unreadable for the whole session.
    match crypto::derive_key(&body.password, &kdf_salt) {
        Ok(key) => keys.insert(token.hash.clone(), key),
        // Not fatal: signing in still works, the drive simply cannot
        // decrypt until they sign in again. Better than refusing the login.
        Err(error) => tracing::error!(?error, user = %id, "could not derive the wrapping key"),
    }

    audit(&pool, Some(id), Some(id), "user.login", None).await;

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
    keys: web::Data<KeyCache>,
) -> ApiResult<HttpResponse> {
    if let Some(cookie) = request.cookie(SESSION_COOKIE) {
        let hash = hash_session_token(cookie.value());
        // Drop the key first. If the delete below fails, the worse outcome
        // is a live row whose key is gone — recoverable by signing in again
        // — rather than a dead row whose key lingers in memory.
        keys.remove(&hash);
        let client = pool.get().await?;
        client
            .execute("DELETE FROM app_sessions WHERE token_hash = $1", &[&hash])
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
/// encrypted under a key derived from the password, so changing the password
/// without re-wrapping would leave a row nothing can ever decrypt — the
/// user's Telegram connection would be silently destroyed and they would
/// only find out the next time they used it.
async fn change_password(
    user: CurrentUser,
    body: web::Json<PasswordChange>,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
) -> ApiResult<HttpResponse> {
    validate_password(&body.new_password).map_err(ApiError::BadRequest)?;

    let mut client = pool.get().await?;

    let row = client
        .query_one(
            "SELECT password_hash, kdf_salt FROM users WHERE id = $1",
            &[&user.id],
        )
        .await?;
    let stored: String = row.get("password_hash");
    let kdf_salt: Vec<u8> = row.get("kdf_salt");

    let ok = verify_password(&body.current_password, &stored)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    if !ok {
        return Err(ApiError::Forbidden("Your current password is wrong.".into()));
    }

    let new_hash = hash_password(&body.new_password)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

    let old_key = crypto::derive_key(&body.current_password, &kdf_salt)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    // A fresh salt as well as a fresh key: nothing encrypted under the old
    // one survives this transaction, so there is no reason to keep it.
    let new_salt = crypto::generate_salt();
    let new_key = crypto::derive_key(&body.new_password, &new_salt)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

    // Everything below has to land together: a password updated without its
    // session re-wrapped is an unrecoverable state.
    let tx = client.transaction().await?;

    let session_row = tx
        .query_opt(
            "SELECT session_enc, session_nonce, api_hash_enc, api_hash_nonce
             FROM tg_sessions WHERE user_id = $1 FOR UPDATE",
            &[&user.id],
        )
        .await?;

    if let Some(row) = session_row {
        let session_enc: Vec<u8> = row.get("session_enc");
        let session_nonce: Vec<u8> = row.get("session_nonce");
        let api_hash_enc: Vec<u8> = row.get("api_hash_enc");
        let api_hash_nonce: Vec<u8> = row.get("api_hash_nonce");

        // A failure here means the stored session was already unreadable,
        // which is a fault worth surfacing rather than papering over by
        // dropping the row.
        let session = crypto::decrypt(&old_key, &session_enc, &session_nonce)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("stored session did not decrypt")))?;
        let api_hash = crypto::decrypt(&old_key, &api_hash_enc, &api_hash_nonce)
            .map_err(|_| ApiError::Internal(anyhow::anyhow!("stored api hash did not decrypt")))?;

        let (session_enc, session_nonce) = crypto::encrypt(&new_key, &session)
            .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
        let (api_hash_enc, api_hash_nonce) = crypto::encrypt(&new_key, &api_hash)
            .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

        tx.execute(
            "UPDATE tg_sessions
             SET session_enc = $1, session_nonce = $2,
                 api_hash_enc = $3, api_hash_nonce = $4
             WHERE user_id = $5",
            &[
                &session_enc,
                &session_nonce,
                &api_hash_enc,
                &api_hash_nonce,
                &user.id,
            ],
        )
        .await?;
    }

    tx.execute(
        "UPDATE users SET password_hash = $1, kdf_salt = $2 WHERE id = $3",
        &[&new_hash, &new_salt.as_slice(), &user.id],
    )
    .await?;

    // Every other login is invalidated: a password change is how someone
    // responds to a session they think is stolen, so leaving the rest alive
    // would defeat the point.
    let revoked = tx
        .query(
            "DELETE FROM app_sessions WHERE user_id = $1 RETURNING token_hash",
            &[&user.id],
        )
        .await?;
    let revoked: Vec<Vec<u8>> = revoked.iter().map(|row| row.get("token_hash")).collect();

    tx.commit().await?;

    // Only after the commit: dropping cached keys for sessions that still
    // exist, because the transaction rolled back, would log the user out of
    // their drive for no reason.
    keys.remove_many(&revoked);

    audit(&pool, Some(user.id), Some(user.id), "user.password_changed", None).await;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "ok": true,
        "message": "Password changed. Please sign in again."
    })))
}

#[derive(Deserialize)]
pub struct Unlock {
    pub password: String,
}

/// Re-derives the wrapping key for an already-signed-in session.
///
/// The key lives only in memory and is lost on restart, so a perfectly
/// valid login can find itself unable to read its own Telegram session.
/// Forcing a full sign-out to recover is both annoying and misleading —
/// the session is fine, it is only the key that is missing. This asks for
/// the password, derives the key again, and carries on.
async fn unlock(
    request: HttpRequest,
    user: CurrentUser,
    body: web::Json<Unlock>,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
) -> ApiResult<HttpResponse> {
    let cookie = request
        .cookie(SESSION_COOKIE)
        .ok_or(ApiError::Unauthenticated)?;

    let client = pool.get().await?;
    let row = client
        .query_one(
            "SELECT password_hash, kdf_salt FROM users WHERE id = $1",
            &[&user.id],
        )
        .await?;
    let stored: String = row.get("password_hash");
    let kdf_salt: Vec<u8> = row.get("kdf_salt");

    let ok = verify_password(&body.password, &stored)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    if !ok {
        return Err(ApiError::Forbidden("That password is wrong.".into()));
    }

    let key = crypto::derive_key(&body.password, &kdf_salt)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    keys.insert(hash_session_token(cookie.value()), key);

    tracing::info!(user = %user.id, "session unlocked");
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

async fn me(user: CurrentUser, pool: web::Data<Db>) -> ApiResult<HttpResponse> {
    let client = pool.get().await?;
    let row = client
        .query_one(
            "SELECT email, role, status FROM users WHERE id = $1",
            &[&user.id],
        )
        .await?;

    let role: String = row.get("role");
    let status: String = row.get("status");

    Ok(HttpResponse::Ok().json(UserView {
        id: user.id,
        email: row.get("email"),
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
    let result = async {
        let client = pool.get().await?;
        client
            .execute(
                "INSERT INTO audit (user_id, actor_id, action, detail)
                 VALUES ($1, $2, $3, $4)",
                &[&user_id, &actor_id, &action, &detail],
            )
            .await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;

    if let Err(error) = result {
        tracing::warn!(?error, action, "could not write audit row");
    }
}
