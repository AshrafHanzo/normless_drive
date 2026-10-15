//! The connect wizard, server side.
//!
//!   POST /api/telegram/start     api_id, api_hash, phone  -> code sent
//!   POST /api/telegram/code      code                     -> done | needs password
//!   POST /api/telegram/password  password                 -> done
//!   GET  /api/telegram/status                             -> connected?
//!   POST /api/telegram/disconnect                         -> forget the session

use actix_web::{web, HttpRequest, HttpResponse};
use serde::{Deserialize, Serialize};

use super::{
    client_from_session, request_code, submit_code, submit_password, CodeOutcome, LoginStage,
    Pending, SharedPending, TelegramError,
};
use crate::{
    auth::{hash_session_token, routes::audit, CurrentUser, SESSION_COOKIE},
    crypto,
    db::Db,
    error::{ApiError, ApiResult},
    keycache::KeyCache,
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/telegram")
            .route("/status", web::get().to(status))
            .route("/start", web::post().to(start))
            .route("/code", web::post().to(code))
            .route("/password", web::post().to(password))
            .route("/disconnect", web::post().to(disconnect)),
    );
}

impl From<TelegramError> for ApiError {
    fn from(error: TelegramError) -> Self {
        match error {
            // Everything a user can fix by typing something different is a
            // 400 with Telegram's meaning translated; only genuinely
            // unexpected failures become a 500.
            TelegramError::Other(detail) => {
                ApiError::Internal(anyhow::anyhow!("telegram: {detail}"))
            }
            other => ApiError::BadRequest(other.to_string()),
        }
    }
}

/// Pulls the caller's wrapping key out of the cache.
///
/// Absent means the server restarted, or the session went idle, since they
/// signed in. Nothing encrypted can be read or written until they sign in
/// again — and saying so plainly beats a decryption error.
fn wrapping_key(
    request: &HttpRequest,
    keys: &KeyCache,
) -> ApiResult<(Vec<u8>, crypto::WrappingKey)> {
    let cookie = request
        .cookie(SESSION_COOKIE)
        .ok_or(ApiError::Unauthenticated)?;
    let hash = hash_session_token(cookie.value());
    let key = keys.get(&hash).ok_or_else(|| {
        ApiError::Forbidden("Please sign in again to unlock your Telegram connection.".into())
    })?;
    Ok((hash, key))
}

#[derive(Serialize)]
struct Status {
    connected: bool,
    phone_hint: Option<String>,
    connected_at: Option<chrono::DateTime<chrono::Utc>>,
    /// False when the key is not cached, i.e. a stored connection exists but
    /// cannot be used until the user signs in again.
    unlocked: bool,
}

async fn status(
    request: HttpRequest,
    user: CurrentUser,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
) -> ApiResult<HttpResponse> {
    let client = pool.get().await?;
    let row = client
        .query_opt(
            "SELECT phone_hint, connected_at FROM tg_sessions WHERE user_id = $1",
            &[&user.id],
        )
        .await?;

    let unlocked = request
        .cookie(SESSION_COOKIE)
        .map(|cookie| hash_session_token(cookie.value()))
        .is_some_and(|hash| keys.get(&hash).is_some());

    Ok(HttpResponse::Ok().json(match row {
        Some(row) => Status {
            connected: true,
            phone_hint: row.get("phone_hint"),
            connected_at: Some(row.get("connected_at")),
            unlocked,
        },
        None => Status {
            connected: false,
            phone_hint: None,
            connected_at: None,
            unlocked,
        },
    }))
}

#[derive(Deserialize)]
pub struct StartRequest {
    pub api_id: i32,
    pub api_hash: String,
    pub phone: String,
}

#[derive(Serialize)]
struct StartResponse {
    sent: bool,
    phone_hint: String,
}

async fn start(
    user: CurrentUser,
    body: web::Json<StartRequest>,
    pending: web::Data<SharedPending>,
) -> ApiResult<HttpResponse> {
    let phone = normalize_phone(&body.phone).map_err(ApiError::BadRequest)?;
    let api_hash = body.api_hash.trim().to_string();

    if body.api_id <= 0 {
        return Err(ApiError::BadRequest(
            "The API ID should be a positive number from my.telegram.org.".into(),
        ));
    }
    // Telegram's hashes are 32 hex characters. Checking here turns a
    // confusing connection failure into a pointer at the wrong field.
    if api_hash.len() != 32 || !api_hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::BadRequest(
            "The API hash should be 32 characters of hex from my.telegram.org.".into(),
        ));
    }

    // Replaces any earlier attempt, closing its connection.
    pending.remove(user.id).await;

    let (client, token) = request_code(body.api_id, &api_hash, &phone).await?;

    pending
        .insert(
            user.id,
            Pending {
                client,
                stage: LoginStage::AwaitingCode(token),
                api_id: body.api_id,
                api_hash,
                phone: phone.clone(),
                started: std::time::Instant::now(),
            },
        )
        .await;

    Ok(HttpResponse::Ok().json(StartResponse {
        sent: true,
        phone_hint: hint(&phone),
    }))
}

#[derive(Deserialize)]
pub struct CodeRequest {
    pub code: String,
}

#[derive(Serialize)]
struct StepResponse {
    connected: bool,
    needs_password: bool,
}

async fn code(
    request: HttpRequest,
    user: CurrentUser,
    body: web::Json<CodeRequest>,
    pending: web::Data<SharedPending>,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
) -> ApiResult<HttpResponse> {
    let entry = pending.take(user.id).await.ok_or_else(|| {
        ApiError::BadRequest("That attempt expired. Start again from your phone number.".into())
    })?;

    let LoginStage::AwaitingCode(ref token) = entry.stage else {
        return Err(ApiError::BadRequest(
            "This attempt is waiting for your two-step password, not a code.".into(),
        ));
    };

    // Telegram codes are digits; people paste them with spaces from SMS.
    let typed: String = body.code.chars().filter(|c| c.is_ascii_digit()).collect();
    if typed.is_empty() {
        pending.insert(user.id, entry).await;
        return Err(ApiError::BadRequest("Enter the code Telegram sent you.".into()));
    }

    match submit_code(&entry.client, token, &typed).await {
        Ok(CodeOutcome::SignedIn(session)) => {
            store_session(
                &request,
                &user,
                entry.api_id,
                &entry.api_hash,
                &entry.phone,
                session,
                &pool,
                &keys,
            )
            .await?;
            Ok(HttpResponse::Ok().json(StepResponse {
                connected: true,
                needs_password: false,
            }))
        }
        Ok(CodeOutcome::NeedsPassword(password_token)) => {
            // Keep the client alive: the password step must run on the same
            // connection that accepted the code.
            pending
                .insert(
                    user.id,
                    Pending {
                        stage: LoginStage::AwaitingPassword(password_token),
                        ..entry
                    },
                )
                .await;
            Ok(HttpResponse::Ok().json(StepResponse {
                connected: false,
                needs_password: true,
            }))
        }
        Err(error) => {
            // A wrong code is worth another go on the same token; an expired
            // one is not, and putting it back would loop the user forever.
            if matches!(error, TelegramError::BadCode) {
                pending.insert(user.id, entry).await;
            }
            Err(error.into())
        }
    }
}

#[derive(Deserialize)]
pub struct PasswordRequest {
    pub password: String,
}

async fn password(
    request: HttpRequest,
    user: CurrentUser,
    body: web::Json<PasswordRequest>,
    pending: web::Data<SharedPending>,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
) -> ApiResult<HttpResponse> {
    let entry = pending.take(user.id).await.ok_or_else(|| {
        ApiError::BadRequest("That attempt expired. Start again from your phone number.".into())
    })?;

    // Destructured rather than rebuilt: `check_password` consumes the token
    // and nothing afterwards needs a stage, so there is no dummy value to
    // invent. A wrong password therefore ends this attempt and the user
    // restarts from the phone number — Telegram's own clients do the same.
    let Pending {
        client,
        stage,
        api_id,
        api_hash,
        phone,
        ..
    } = entry;

    let LoginStage::AwaitingPassword(token) = stage else {
        return Err(ApiError::BadRequest(
            "This attempt is waiting for the code Telegram sent, not a password.".into(),
        ));
    };

    let session = submit_password(&client, token, &body.password).await?;
    store_session(&request, &user, api_id, &api_hash, &phone, session, &pool, &keys).await?;

    Ok(HttpResponse::Ok().json(StepResponse {
        connected: true,
        needs_password: false,
    }))
}

async fn disconnect(
    user: CurrentUser,
    pool: web::Data<Db>,
    pending: web::Data<SharedPending>,
) -> ApiResult<HttpResponse> {
    pending.remove(user.id).await;

    let client = pool.get().await?;
    let affected = client
        .execute("DELETE FROM tg_sessions WHERE user_id = $1", &[&user.id])
        .await?;

    audit(&pool, Some(user.id), Some(user.id), "telegram.disconnect", None).await;

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "ok": true,
        "was_connected": affected > 0
    })))
}

// ------------------------------------------------------------------ helpers

/// Encrypts the session and api_hash under the caller's wrapping key and
/// writes them, replacing any previous connection.
#[allow(clippy::too_many_arguments)]
async fn store_session(
    request: &HttpRequest,
    user: &CurrentUser,
    api_id: i32,
    api_hash: &str,
    phone: &str,
    session: Vec<u8>,
    pool: &Db,
    keys: &KeyCache,
) -> ApiResult<()> {
    let (_, key) = wrapping_key(request, keys)?;

    let (session_enc, session_nonce) =
        crypto::encrypt(&key, &session).map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;
    let (api_hash_enc, api_hash_nonce) = crypto::encrypt(&key, api_hash.as_bytes())
        .map_err(|e| ApiError::Internal(anyhow::anyhow!(e)))?;

    let client = pool.get().await?;
    client
        .execute(
            "INSERT INTO tg_sessions
                (user_id, api_id, api_hash_enc, api_hash_nonce,
                 session_enc, session_nonce, kdf_salt, phone_hint, connected_at)
             VALUES ($1, $2, $3, $4, $5, $6, ''::bytea, $7, now())
             ON CONFLICT (user_id) DO UPDATE SET
                api_id = EXCLUDED.api_id,
                api_hash_enc = EXCLUDED.api_hash_enc,
                api_hash_nonce = EXCLUDED.api_hash_nonce,
                session_enc = EXCLUDED.session_enc,
                session_nonce = EXCLUDED.session_nonce,
                phone_hint = EXCLUDED.phone_hint,
                connected_at = now()",
            &[
                &user.id,
                &api_id,
                &api_hash_enc,
                &api_hash_nonce,
                &session_enc,
                &session_nonce,
                &hint(phone),
            ],
        )
        .await?;

    audit(pool, Some(user.id), Some(user.id), "telegram.connected", None).await;
    tracing::info!(user = %user.id, "telegram connected");
    Ok(())
}

/// Keeps only enough of the number to be recognisable: `+91 ••••• ••123`.
/// Stored rather than the full number — the drive has no use for it and a
/// phone number is personal data worth not keeping.
fn hint(phone: &str) -> String {
    let digits: Vec<char> = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    let tail: String = digits.iter().rev().take(3).rev().collect();
    format!("•••••{tail}")
}

fn normalize_phone(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();

    if digits.len() < 7 || digits.len() > 15 {
        return Err("That does not look like a phone number. Include the country code.".into());
    }
    // grammers wants it with the leading +.
    Ok(format!("+{digits}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_number_keeps_only_its_digits_and_gains_a_plus() {
        assert_eq!(normalize_phone(" +91 98765 43210 ").unwrap(), "+919876543210");
        assert_eq!(normalize_phone("(044) 1234-5678").unwrap(), "+04412345678");
    }

    #[test]
    fn an_implausible_phone_number_is_refused() {
        for bad in ["", "123", "abc", &"9".repeat(16)] {
            assert!(normalize_phone(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn the_stored_hint_reveals_only_the_last_three_digits() {
        // Enough for the owner to recognise which number they used, not
        // enough to be worth stealing.
        assert_eq!(hint("+919876543210"), "•••••210");
        assert_eq!(hint("+1 555 0100"), "•••••100");
    }
}
