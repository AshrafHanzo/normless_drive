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
    request_code, submit_code, submit_password, CodeOutcome, LoginStage, Pending, SharedPending,
    TelegramError,
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
            .route("/qr/start", web::post().to(qr_start))
            .route("/qr/poll", web::post().to(qr_poll))
            .route("/start", web::post().to(start))
            .route("/code", web::post().to(code))
            .route("/resend", web::post().to(resend))
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
    let key = keys.get(&hash).ok_or(ApiError::SessionLocked)?;
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
    /// Where Telegram actually sent it, in words for the user.
    delivery: String,
    /// False when the code is a word or phrase rather than digits, so the
    /// input must not filter to numbers.
    numeric: bool,
    /// What a resend would switch to, if Telegram offers an alternative.
    next: Option<String>,
}

async fn start(
    user: CurrentUser,
    body: web::Json<StartRequest>,
    pending: web::Data<SharedPending>,
) -> ApiResult<HttpResponse> {
    let phone = normalize_phone(&body.phone).map_err(ApiError::BadRequest)?;
    let api_hash = body.api_hash.trim().to_string();

    validate_credentials(body.api_id, &api_hash)?;

    // Replaces any earlier attempt, closing its connection.
    pending.remove(user.id).await;

    let session = std::sync::Arc::new(grammers_session::storages::MemorySession::from(
        grammers_session::SessionData::default(),
    ));
    let connection = super::Connection::open(session, body.api_id);

    let sent = match super::phone::send_code(&connection, body.api_id, &api_hash, &phone)
        .await
    {
        Ok(sent) => sent,
        Err(error) => {
            connection.close().await;
            return Err(error.into());
        }
    };

    // Telegram picked a method this app cannot finish. Saying so now beats
    // leaving someone watching an inbox for a code that will never come.
    if !sent.delivery.usable() {
        connection.close().await;
        return Err(ApiError::BadRequest(sent.delivery.describe()));
    }

    let delivery = sent.delivery.clone();
    pending
        .insert(
            user.id,
            Pending {
                connection,
                stage: LoginStage::AwaitingCode {
                    phone_code_hash: sent.phone_code_hash,
                    delivery: delivery.clone(),
                },
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
        delivery: delivery.describe(),
        numeric: delivery.numeric(),
        next: sent.next.map(|m| m.describe().to_string()),
    }))
}

#[derive(Deserialize)]
pub struct QrStart {
    pub api_id: i32,
    pub api_hash: String,
}

#[derive(Serialize)]
struct QrResponse {
    /// `tg://login?token=…`, to be rendered as a QR code by the browser.
    url: Option<String>,
    expires: Option<i32>,
    connected: bool,
}

/// Opens a connection and asks for a login token to display.
async fn qr_start(
    user: CurrentUser,
    body: web::Json<QrStart>,
    pending: web::Data<SharedPending>,
) -> ApiResult<HttpResponse> {
    let api_hash = body.api_hash.trim().to_string();
    validate_credentials(body.api_id, &api_hash)?;

    pending.remove(user.id).await;

    let session = std::sync::Arc::new(grammers_session::storages::MemorySession::from(
        grammers_session::SessionData::default(),
    ));
    let connection = super::Connection::open(session, body.api_id);

    let state = match super::qr::export(&connection.client, body.api_id, &api_hash).await {
        Ok(state) => state,
        Err(error) => {
            connection.close().await;
            return Err(error.into());
        }
    };

    let (url, expires) = match state {
        super::qr::QrState::Waiting { token, expires } => {
            (Some(super::qr::login_url(&token)), Some(expires))
        }
        // Neither can happen on the first call — nothing has been scanned
        // yet — but answering honestly beats inventing a token.
        super::qr::QrState::Authorized | super::qr::QrState::Migrate { .. } => (None, None),
    };

    pending
        .insert(
            user.id,
            Pending {
                connection,
                stage: LoginStage::AwaitingQrScan,
                api_id: body.api_id,
                api_hash,
                phone: String::new(),
                started: std::time::Instant::now(),
            },
        )
        .await;

    Ok(HttpResponse::Ok().json(QrResponse {
        url,
        expires,
        connected: false,
    }))
}

/// Polls for a scan. The browser calls this every couple of seconds.
async fn qr_poll(
    request: HttpRequest,
    user: CurrentUser,
    pending: web::Data<SharedPending>,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
) -> ApiResult<HttpResponse> {
    // A scan triggers a datacenter handshake that can take seconds. The
    // browser keeps polling on its timer regardless, and without this the
    // overlapping poll finds the entry checked out and declares the
    // sign-in expired — cancelling one that was about to succeed.
    if !pending.begin_poll(user.id) {
        return Ok(HttpResponse::Ok().json(QrResponse {
            url: None,
            expires: None,
            connected: false,
        }));
    }

    let outcome = qr_poll_inner(&request, &user, &pending, &pool, &keys).await;
    pending.end_poll(user.id);
    outcome
}

async fn qr_poll_inner(
    request: &HttpRequest,
    user: &CurrentUser,
    pending: &SharedPending,
    pool: &Db,
    keys: &KeyCache,
) -> ApiResult<HttpResponse> {
    let entry = pending.take(user.id).await.ok_or_else(|| {
        ApiError::BadRequest("That sign-in expired. Show a new QR code.".into())
    })?;

    if !matches!(entry.stage, LoginStage::AwaitingQrScan) {
        pending.insert(user.id, entry).await;
        return Err(ApiError::BadRequest(
            "This attempt is not a QR sign-in.".into(),
        ));
    }

    let state = super::qr::export(&entry.connection.client, entry.api_id, &entry.api_hash).await;

    let state = match state {
        Ok(state) => state,
        Err(error) => {
            entry.connection.close().await;
            return Err(error.into());
        }
    };

    match state {
        super::qr::QrState::Waiting { token, expires } => {
            let url = super::qr::login_url(&token);
            pending.insert(user.id, entry).await;
            Ok(HttpResponse::Ok().json(QrResponse {
                url: Some(url),
                expires: Some(expires),
                connected: false,
            }))
        }
        super::qr::QrState::Migrate { dc_id, token } => {
            // A MigrateTo reply means the scan SUCCEEDED and the account
            // lives in another datacenter. The token it carries is only
            // valid against that datacenter, so the connection has to move
            // first — importing it here answers AUTH_TOKEN_EXPIRED, which
            // reads as "the QR code timed out" when in fact it worked.
            tracing::info!(dc_id, "QR scanned; moving to the account's datacenter");
            entry.connection.migrate_to(dc_id);
            let imported = super::qr::import(&entry.connection.client, token).await;
            match imported {
                Ok(super::qr::QrState::Authorized) => {
                    finish_qr(&request, &user, entry, &pool, &keys).await
                }
                Ok(_) => {
                    pending.insert(user.id, entry).await;
                    Ok(HttpResponse::Ok().json(QrResponse {
                        url: None,
                        expires: None,
                        connected: false,
                    }))
                }
                Err(error) => {
                    entry.connection.close().await;
                    Err(error.into())
                }
            }
        }
        super::qr::QrState::Authorized => finish_qr(&request, &user, entry, &pool, &keys).await,
    }
}

/// Saves the session once a QR login has been accepted.
async fn finish_qr(
    request: &HttpRequest,
    user: &CurrentUser,
    entry: Pending,
    pool: &Db,
    keys: &KeyCache,
) -> ApiResult<HttpResponse> {
    let session = super::session_store::export(&entry.connection.session)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!("{error}")))?;

    let stored = store_session(
        request,
        user,
        entry.api_id,
        &entry.api_hash,
        // QR login never learns the phone number, so there is nothing to
        // hint at. Saying "QR" is more honest than inventing digits.
        "qr",
        session,
        pool,
        keys,
    )
    .await;

    entry.connection.close().await;
    stored?;

    Ok(HttpResponse::Ok().json(QrResponse {
        url: None,
        expires: None,
        connected: true,
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

    let LoginStage::AwaitingCode {
        ref phone_code_hash,
        ref delivery,
    } = entry.stage
    else {
        return Err(ApiError::BadRequest(
            "This attempt is waiting for your two-step password, not a code.".into(),
        ));
    };

    // Only strip to digits when the code actually is digits. Telegram's
    // word and phrase codes would otherwise be erased entirely, leaving
    // the user unable to submit anything at all.
    let typed: String = if delivery.numeric() {
        body.code.chars().filter(|c| c.is_ascii_digit()).collect()
    } else {
        body.code.trim().to_string()
    };

    if typed.is_empty() {
        let hash = phone_code_hash.clone();
        let kind = delivery.clone();
        pending
            .insert(
                user.id,
                Pending {
                    stage: LoginStage::AwaitingCode {
                        phone_code_hash: hash,
                        delivery: kind,
                    },
                    ..entry
                },
            )
            .await;
        return Err(ApiError::BadRequest("Enter the code Telegram sent you.".into()));
    }

    let outcome = super::phone::sign_in(
        &entry.connection.client,
        &entry.phone,
        phone_code_hash,
        &typed,
    )
    .await
    .map(|result| match result {
        super::phone::SignIn::Done => CodeOutcome::SignedIn(Vec::new()),
        super::phone::SignIn::NeedsPassword(token) => CodeOutcome::NeedsPassword(token),
    });

    match outcome {
        Ok(CodeOutcome::SignedIn(_)) => {
            // Raw sign_in authorises the connection but returns nothing to
            // store; the session has to be exported from the client that
            // was just authorised.
            let session = super::session_store::export(&entry.connection.session)
                .map_err(|error| ApiError::Internal(anyhow::anyhow!("{error}")))?;
            let result = store_session(
                &request,
                &user,
                entry.api_id,
                &entry.api_hash,
                &entry.phone,
                session,
                &pool,
                &keys,
            )
            .await;

            // Done with Telegram either way: the session is saved, so this
            // connection has no further use and must not be left holding a
            // socket and a spawned task.
            entry.connection.close().await;
            result?;

            Ok(HttpResponse::Ok().json(StepResponse {
                connected: true,
                needs_password: false,
            }))
        }
        Ok(CodeOutcome::NeedsPassword(password_token)) => {
            // Keep the connection alive: the password step must run on the
            // same one that accepted the code.
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
            // A mistyped code is worth another go on the same token. Any
            // other failure ends the attempt, and putting it back would
            // loop the user forever — so close it.
            if matches!(error, TelegramError::BadCode) {
                pending.insert(user.id, entry).await;
            } else {
                entry.connection.close().await;
            }
            Err(error.into())
        }
    }
}

/// Asks Telegram for the code again, by whatever method it offers next.
///
/// Usually the way to get an SMS: Telegram's first choice is to push the
/// code into the app, which is no use to someone who is not already
/// signed in on another device.
async fn resend(
    user: CurrentUser,
    pending: web::Data<SharedPending>,
) -> ApiResult<HttpResponse> {
    let entry = pending.take(user.id).await.ok_or_else(|| {
        ApiError::BadRequest("That attempt expired. Start again from your phone number.".into())
    })?;

    let LoginStage::AwaitingCode {
        ref phone_code_hash,
        ..
    } = entry.stage
    else {
        pending.insert(user.id, entry).await;
        return Err(ApiError::BadRequest(
            "There is no code to resend right now.".into(),
        ));
    };

    let sent =
        match super::phone::resend(&entry.connection.client, &entry.phone, phone_code_hash).await {
            Ok(sent) => sent,
            Err(error) => {
                entry.connection.close().await;
                return Err(error.into());
            }
        };

    let delivery = sent.delivery.clone();
    let next = sent.next;

    pending
        .insert(
            user.id,
            Pending {
                stage: LoginStage::AwaitingCode {
                    // Telegram can issue a new hash on resend. Keeping the
                    // old one would fail the next submission as an invalid
                    // code, which reads as the user mistyping.
                    phone_code_hash: sent.phone_code_hash,
                    delivery: delivery.clone(),
                },
                ..entry
            },
        )
        .await;

    Ok(HttpResponse::Ok().json(StartResponse {
        sent: true,
        phone_hint: hint(&entry_phone_placeholder()),
        delivery: delivery.describe(),
        numeric: delivery.numeric(),
        next: next.map(|method| method.describe().to_string()),
    }))
}

/// The hint is not meaningful on a resend — the number has not changed —
/// but the response shape is shared, so this keeps it honest rather than
/// echoing a stale value.
fn entry_phone_placeholder() -> String {
    String::new()
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
        connection,
        stage,
        api_id,
        api_hash,
        phone,
        ..
    } = entry;

    let LoginStage::AwaitingPassword(token) = stage else {
        // Wrong step. Close rather than drop, or the pool task outlives the
        // request that owned it.
        connection.close().await;
        return Err(ApiError::BadRequest(
            "This attempt is waiting for the code Telegram sent, not a password.".into(),
        ));
    };

    let signed_in = submit_password(&connection, *token, &body.password).await;

    let stored = match signed_in {
        Ok(session) => {
            store_session(&request, &user, api_id, &api_hash, &phone, session, &pool, &keys).await
        }
        Err(error) => Err(error.into()),
    };

    // The token is consumed either way, so this connection is finished
    // whether the password was right or not.
    connection.close().await;
    stored?;

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

/// Keeps only enough of the number to be recognisable: `...506`.
///
/// Stored rather than the full number — the drive has no use for it and a
/// phone number is personal data worth not keeping.
///
/// Deliberately ASCII. This string is written to the database and rendered
/// in the browser, and a decorative bullet bought nothing while giving
/// every tool in the chain a chance to mangle the encoding — which is
/// exactly what happened.
fn hint(phone: &str) -> String {
    let digits: Vec<char> = phone.chars().filter(|c| c.is_ascii_digit()).collect();
    let tail: String = digits.iter().rev().take(3).rev().collect();
    format!("...{tail}")
}

/// Catches a mistyped API ID or hash before a connection is attempted.
///
/// Both failures otherwise surface as an opaque connection error, which
/// points at the network rather than at the field that is actually wrong.
fn validate_credentials(api_id: i32, api_hash: &str) -> ApiResult<()> {
    if api_id <= 0 {
        return Err(ApiError::BadRequest(
            "The API ID should be a positive number from my.telegram.org.".into(),
        ));
    }
    // Telegram's hashes are exactly 32 hex characters.
    if api_hash.len() != 32 || !api_hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::BadRequest(
            "The API hash should be 32 characters of hex from my.telegram.org.".into(),
        ));
    }
    Ok(())
}

/// Normalises a phone number to `+<digits>`.
///
/// Requires an explicit `+` or `00` prefix. The earlier version simply
/// prefixed `+` to whatever digits it was given, which silently turned a
/// local number like `7305041971` into `+7305041971` — a Russian prefix.
/// Telegram accepts `sendCode` for that happily and reports success, so
/// the only symptom was a code that never arrived, with nothing anywhere
/// indicating why.
fn normalize_phone(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();

    let explicit = trimmed.starts_with('+') || trimmed.replace(['(', ' ', '-'], "").starts_with("00");
    if !explicit {
        return Err(
            "Start with your country code, like +91 7305041971. Without it we cannot tell \
             which country the number belongs to, and the code would be sent to the wrong one."
                .into(),
        );
    }

    let mut digits: String = trimmed.chars().filter(|c| c.is_ascii_digit()).collect();
    // 00 is the international prefix in much of the world; Telegram wants
    // the + form, and the two mean the same thing.
    if !trimmed.starts_with('+') && digits.starts_with("00") {
        digits = digits[2..].to_string();
    }

    // ITU E.164: a country code plus a subscriber number is at least 8
    // digits in practice and never more than 15.
    if digits.len() < 8 || digits.len() > 15 {
        return Err("That does not look like a full phone number.".into());
    }

    Ok(format!("+{digits}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_number_with_an_explicit_country_code_is_normalised() {
        assert_eq!(normalize_phone(" +91 73050 41971 ").unwrap(), "+917305041971");
        assert_eq!(normalize_phone("+1 (555) 010-0123").unwrap(), "+15550100123");
        // 00 is the same thing as + in most of the world.
        assert_eq!(normalize_phone("0091 7305041971").unwrap(), "+917305041971");
    }

    #[test]
    fn a_number_without_a_country_code_is_refused() {
        // This is the bug that cost real debugging time: "7305041971" was
        // being turned into "+7305041971", a Russian prefix. Telegram
        // accepted it and reported success, and the code went nowhere the
        // user could see.
        let error = normalize_phone("7305041971").expect_err("must be refused");
        assert!(error.contains("country code"), "got: {error}");

        for bare in ["9876543210", "555 010 0123", "(044) 1234-5678"] {
            assert!(normalize_phone(bare).is_err(), "should refuse {bare:?}");
        }
    }

    #[test]
    fn an_implausible_phone_number_is_refused() {
        for bad in ["", "+123", "abc", "+", &format!("+{}", "9".repeat(16))] {
            assert!(normalize_phone(bad).is_err(), "should reject {bad:?}");
        }
    }

    #[test]
    fn the_stored_hint_reveals_only_the_last_three_digits() {
        // Enough for the owner to recognise which number they used, not
        // enough to be worth stealing.
        assert_eq!(hint("+919876543210"), "...210");
        assert_eq!(hint("+1 555 0100"), "...100");
    }

    #[test]
    fn the_hint_is_pure_ascii() {
        // It crosses Rust, Postgres, JSON and the browser. Every non-ASCII
        // character in that chain is a chance for one of them to mangle it,
        // and one already did.
        assert!(hint("+919876543210").is_ascii());
    }
}
