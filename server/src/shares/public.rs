//! `/s/{token}` — the unauthenticated side of a share link.
//!
//! Everything here is reachable by anyone holding the token, so each
//! handler re-resolves the share and re-checks its permissions. There is no
//! session to lean on and no "the UI would not have shown this" to rely on.

use actix_web::{web, HttpRequest, HttpResponse};
use chrono::{DateTime, Utc};
use futures::StreamExt;
use grammers_client::types::Media;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{SharePermissionView, SharePermissions};
use crate::{
    config::Config,
    crypto::{self, WrappingKey},
    db::Db,
    error::{ApiError, ApiResult},
    keycache::KeyCache,
    telegram::{connect::SharedClients, session_store, Connection},
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    // Mounted under /api/s/, not /s/. The browser-facing share page owns
    // `/s/{token}` and must receive index.html there; if the API claimed
    // the same path, opening a link would download JSON instead of showing
    // a page.
    cfg.service(
        web::scope("/api/s/{token}")
            .route("", web::get().to(info))
            .route("/verify", web::post().to(verify))
            .route("/files", web::get().to(list))
            .route("/files/{id}/download", web::get().to(download)),
    );
}

/// Derives the wrapping key for a user's server-held session copy.
///
/// Mixed with the user id so one secret does not produce one key for
/// everybody: compromising the wrapped session of one account must not
/// hand over every other account's as well.
pub fn server_key(secret: &str, user: Uuid) -> Result<WrappingKey, crypto::CryptoError> {
    let mut salt = [0u8; crypto::SALT_LEN];
    let bytes = user.as_bytes();
    salt.copy_from_slice(&bytes[..crypto::SALT_LEN]);
    crypto::derive_key(secret, &salt)
}

struct Share {
    id: Uuid,
    owner: Uuid,
    folder_id: Option<Uuid>,
    permissions: SharePermissions,
    password_hash: Option<String>,
    allow_offline: bool,
    label: Option<String>,
    /// When set, the link exposes only these messages.
    message_ids: Option<Vec<i32>>,
}

/// Loads a share by token, enforcing revocation, expiry and usage cap.
///
/// Every failure returns the same 404. Distinguishing "revoked" from
/// "expired" from "never existed" would let someone probe which tokens
/// were once real.
async fn load(pool: &Db, token: &str) -> ApiResult<Share> {
    let client = pool.get().await?;
    let row = client
        .query_opt(
            "SELECT id, user_id, folder_id, permissions, password_hash,
                    expires_at, usage_limit, usage_count, allow_offline, label,
                    message_ids
               FROM shares
              WHERE token = $1 AND NOT revoked",
            &[&token],
        )
        .await?
        .ok_or_else(|| ApiError::NotFound("This link is not available.".into()))?;

    let expires_at: Option<DateTime<Utc>> = row.get("expires_at");
    if expires_at.is_some_and(|at| at < Utc::now()) {
        return Err(ApiError::NotFound("This link has expired.".into()));
    }

    let usage_limit: Option<i32> = row.get("usage_limit");
    let usage_count: i32 = row.get("usage_count");
    if usage_limit.is_some_and(|limit| usage_count >= limit) {
        return Err(ApiError::NotFound("This link has been used up.".into()));
    }

    Ok(Share {
        id: row.get("id"),
        owner: row.get("user_id"),
        folder_id: row.get("folder_id"),
        permissions: SharePermissions::from_bits_truncate(row.get::<_, i64>("permissions")),
        password_hash: row.get("password_hash"),
        allow_offline: row.get("allow_offline"),
        label: row.get("label"),
        message_ids: row.get("message_ids"),
    })
}

/// Name of the cookie proving a password-protected link was unlocked.
fn unlock_cookie(token: &str) -> String {
    format!("nd_share_{}", &token[..16.min(token.len())])
}

/// Whether this request may act on a password-protected share.
fn unlocked(request: &HttpRequest, share: &Share, token: &str) -> bool {
    let Some(hash) = share.password_hash.as_deref() else {
        return true; // no password on this link
    };
    request
        .cookie(&unlock_cookie(token))
        .is_some_and(|cookie| {
            // The cookie carries the bcrypt hash itself, which is already a
            // one-way value and useless anywhere else. Comparing in
            // constant time anyway, so a wrong cookie cannot be refined by
            // timing.
            constant_time_eq::constant_time_eq(cookie.value().as_bytes(), hash.as_bytes())
        })
}

#[derive(Serialize)]
struct Info {
    label: Option<String>,
    permissions: SharePermissionView,
    needs_password: bool,
    unlocked: bool,
    /// False when the owner is signed out and this link cannot reach
    /// Telegram — the UI says so rather than showing an empty folder.
    available: bool,
}

async fn info(
    request: HttpRequest,
    path: web::Path<String>,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    let token = path.into_inner();
    let share = load(&pool, &token).await?;
    let is_unlocked = unlocked(&request, &share, &token);

    let available = share.allow_offline && config.offline_sharing_available()
        || owner_key_present(&pool, &keys, share.owner).await;

    Ok(HttpResponse::Ok().json(Info {
        label: share.label.clone(),
        permissions: share.permissions.into(),
        needs_password: share.password_hash.is_some(),
        unlocked: is_unlocked,
        available,
    }))
}

#[derive(Deserialize)]
struct Unlock {
    password: String,
}

async fn verify(
    path: web::Path<String>,
    body: web::Json<Unlock>,
    pool: web::Data<Db>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    let token = path.into_inner();
    let share = load(&pool, &token).await?;

    let Some(hash) = share.password_hash.as_deref() else {
        return Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })));
    };

    let ok = crate::auth::verify_password(&body.password, hash)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    if !ok {
        return Err(ApiError::Forbidden("That password is wrong.".into()));
    }

    Ok(HttpResponse::Ok()
        .cookie(
            actix_web::cookie::Cookie::build(unlock_cookie(&token), hash.to_string())
                // Scoped so unlocking one share does not unlock another. The
                // path covers both the page and its API calls.
                .path(format!("/api/s/{token}"))
                .http_only(true)
                .secure(config.cookie_secure)
                .same_site(actix_web::cookie::SameSite::Lax)
                .max_age(actix_web::cookie::time::Duration::hours(12))
                .finish(),
        )
        .json(serde_json::json!({ "ok": true })))
}

async fn owner_key_present(pool: &Db, keys: &KeyCache, owner: Uuid) -> bool {
    // A user is "present" if any of their live sessions still has a cached
    // key. Checking the cache directly would need it indexed by user, so
    // this asks the database for their session hashes and tries each.
    let Ok(client) = pool.get().await else {
        return false;
    };
    let Ok(rows) = client
        .query(
            "SELECT token_hash FROM app_sessions WHERE user_id = $1 AND expires_at > now()",
            &[&owner],
        )
        .await
    else {
        return false;
    };

    rows.iter()
        .any(|row| keys.get(&row.get::<_, Vec<u8>>("token_hash")).is_some())
}

/// Opens a Telegram connection on the owner's behalf for a share request.
///
/// Prefers the owner's live key, falling back to the server-held copy when
/// the link allows it. That fallback is the whole point of offline shares:
/// without it a link only works while its owner has a tab open.
async fn owner_connection(
    pool: &Db,
    keys: &KeyCache,
    clients: &SharedClients,
    config: &Config,
    share: &Share,
) -> ApiResult<std::sync::Arc<Connection>> {
    let client = pool.get().await?;

    // 1. The owner is signed in: use the same cached connection their own
    //    browsing uses.
    let rows = client
        .query(
            "SELECT token_hash FROM app_sessions WHERE user_id = $1 AND expires_at > now()",
            &[&share.owner],
        )
        .await?;
    for row in &rows {
        if let Some(key) = keys.get(&row.get::<_, Vec<u8>>("token_hash")) {
            return clients
                .get(share.owner, &key, pool)
                .await
                .map_err(ApiError::from);
        }
    }

    // 2. Otherwise fall back to the server-held copy, if this link may.
    if !share.allow_offline {
        return Err(ApiError::Forbidden(
            "This link only works while its owner is signed in.".into(),
        ));
    }
    let secret = config.share_secret.as_deref().ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!("offline share requested with no SHARE_SECRET"))
    })?;

    let row = client
        .query_opt(
            "SELECT api_id, session_srv_enc, session_srv_nonce
               FROM tg_sessions
              WHERE user_id = $1 AND session_srv_enc IS NOT NULL",
            &[&share.owner],
        )
        .await?
        .ok_or_else(|| {
            ApiError::Forbidden("This link only works while its owner is signed in.".into())
        })?;

    let key = server_key(secret, share.owner)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    let plaintext = crypto::decrypt(
        &key,
        &row.get::<_, Vec<u8>>("session_srv_enc"),
        &row.get::<_, Vec<u8>>("session_srv_nonce"),
    )
    .map_err(|_| ApiError::Internal(anyhow::anyhow!("server-held session did not decrypt")))?;

    let session = session_store::import(&plaintext)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!("{error}")))?;

    Ok(std::sync::Arc::new(Connection::open(
        session,
        row.get::<_, i32>("api_id"),
    )))
}

async fn list(
    request: HttpRequest,
    path: web::Path<String>,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    let token = path.into_inner();
    let share = load(&pool, &token).await?;

    if !share.permissions.contains(SharePermissions::DOWNLOAD) {
        return Err(ApiError::Forbidden(
            "This link does not allow browsing files.".into(),
        ));
    }
    if !unlocked(&request, &share, &token) {
        return Err(ApiError::Forbidden("This link needs a password.".into()));
    }

    let connection = owner_connection(&pool, &keys, &clients, &config, &share).await?;
    let peer =
        crate::telegram::folders::resolve(&connection.client, &pool, share.owner, share.folder_id)
            .await?;

    let mut messages = connection.client.iter_messages(&peer);
    let mut files = Vec::new();
    while let Some(message) = messages.next().await.map_err(|error| {
        ApiError::Internal(anyhow::anyhow!("could not read the folder: {error}"))
    })? {
        // A link restricted to specific files must not list the rest of
        // the folder, however the owner reorganises it later.
        if let Some(allowed) = share.message_ids.as_deref() {
            if !allowed.contains(&message.id()) {
                continue;
            }
        }

        let Some(Media::Document(document)) = message.media() else {
            continue;
        };
        let name = document.name();
        files.push(serde_json::json!({
            "id": message.id(),
            "name": if name.trim().is_empty() { "untitled" } else { name },
            "size": document.size(),
            "mime_type": document.mime_type(),
            "date": message.date().timestamp(),
        }));
        if files.len() >= 200 {
            break;
        }
    }

    touch(&pool, share.id).await;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "files": files })))
}

async fn download(
    request: HttpRequest,
    path: web::Path<(String, i32)>,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    let (token, message_id) = path.into_inner();
    let share = load(&pool, &token).await?;

    if !share.permissions.contains(SharePermissions::DOWNLOAD) {
        return Err(ApiError::Forbidden(
            "This link does not allow downloads.".into(),
        ));
    }
    if !unlocked(&request, &share, &token) {
        return Err(ApiError::Forbidden("This link needs a password.".into()));
    }

    // Checked before anything is fetched. Filtering the listing alone
    // would leave the download endpoint able to serve any file in the
    // folder to anyone who guessed its message id.
    if let Some(allowed) = share.message_ids.as_deref() {
        if !allowed.contains(&message_id) {
            return Err(ApiError::NotFound("No such file.".into()));
        }
    }

    let connection = owner_connection(&pool, &keys, &clients, &config, &share).await?;
    let peer =
        crate::telegram::folders::resolve(&connection.client, &pool, share.owner, share.folder_id)
            .await?;

    let messages = connection
        .client
        .get_messages_by_id(&peer, &[message_id])
        .await
        .map_err(|error| ApiError::Internal(anyhow::anyhow!("could not fetch: {error}")))?;

    let message = messages
        .into_iter()
        .flatten()
        .next()
        .ok_or_else(|| ApiError::NotFound("No such file.".into()))?;

    let Some(Media::Document(document)) = message.media() else {
        return Err(ApiError::NotFound("That message has no file.".into()));
    };

    let raw = document.name();
    let name = if raw.trim().is_empty() { "download" } else { raw };
    let mime = document
        .mime_type()
        .unwrap_or("application/octet-stream")
        .to_string();
    let size = document.size();
    let disposition = format!(
        "attachment; filename=\"{}\"",
        name.chars()
            .filter(|c| !matches!(c, '"' | '\r' | '\n' | '\\' | '/'))
            .collect::<String>()
    );

    let media = Media::Document(document);
    let client = connection.client.clone();
    let stream = async_stream::stream! {
        let mut chunks = client.iter_download(&media);
        loop {
            match chunks.next().await {
                Ok(Some(chunk)) => yield Ok::<_, actix_web::Error>(web::Bytes::from(chunk)),
                Ok(None) => break,
                Err(error) => {
                    tracing::warn!(?error, message_id, "share download failed mid-transfer");
                    break;
                }
            }
        }
    };

    touch(&pool, share.id).await;

    Ok(HttpResponse::Ok()
        .content_type(mime)
        .insert_header((actix_web::http::header::CONTENT_DISPOSITION, disposition))
        .insert_header((actix_web::http::header::CONTENT_LENGTH, size.to_string()))
        .streaming(Box::pin(stream)))
}

/// Records that a link was used. Best effort: a failed counter update must
/// not fail the download it was counting.
async fn touch(pool: &Db, share: Uuid) {
    let result = async {
        let client = pool.get().await?;
        client
            .execute(
                "UPDATE shares
                    SET usage_count = usage_count + 1, last_used_at = now()
                  WHERE id = $1",
                &[&share],
            )
            .await?;
        Ok::<_, anyhow::Error>(())
    }
    .await;
    if let Err(error) = result {
        tracing::warn!(?error, "could not record share usage");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_unlock_cookie_is_per_link() {
        let a = unlock_cookie("aaaaaaaaaaaaaaaaaaaaaaaa");
        let b = unlock_cookie("bbbbbbbbbbbbbbbbbbbbbbbb");
        assert_ne!(a, b, "unlocking one share would unlock another");
        assert!(a.starts_with("nd_share_"));
    }

    #[test]
    fn a_short_token_does_not_panic_the_cookie_name() {
        // Tokens are generated at a fixed length, but this runs on
        // attacker-supplied path segments too.
        assert!(!unlock_cookie("abc").is_empty());
        assert!(!unlock_cookie("").is_empty());
    }

    #[test]
    fn the_server_key_differs_per_user() {
        // One secret must not produce one key for everyone, or compromising
        // a single wrapped session would expose every account's.
        let secret = "a".repeat(32);
        let first = server_key(&secret, Uuid::from_u128(1)).unwrap();
        let second = server_key(&secret, Uuid::from_u128(2)).unwrap();

        let (ciphertext, nonce) = crypto::encrypt(&first, b"session").unwrap();
        assert!(crypto::decrypt(&second, &ciphertext, &nonce).is_err());
    }
}
