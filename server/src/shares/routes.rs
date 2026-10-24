//! Managing your own share links.

use actix_web::{web, HttpRequest, HttpResponse};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{generate_token, SharePermissionView, SharePermissions};
use crate::{
    auth::{hash_session_token, routes::audit, CurrentUser, SESSION_COOKIE},
    config::Config,
    crypto,
    db::Db,
    error::{ApiError, ApiResult},
    keycache::KeyCache,
    permissions,
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/shares")
            .route("", web::get().to(list))
            .route("", web::post().to(create))
            .route("/offline", web::post().to(enable_offline))
            .route("/{id}", web::delete().to(revoke)),
    );
}

#[derive(Serialize)]
pub struct ShareView {
    pub id: Uuid,
    pub token: String,
    pub url: String,
    pub label: Option<String>,
    pub folder_id: Option<Uuid>,
    pub permissions: SharePermissionView,
    pub has_password: bool,
    pub expires_at: Option<DateTime<Utc>>,
    pub usage_limit: Option<i32>,
    pub usage_count: i32,
    pub allow_offline: bool,
    /// How many individual files this link exposes; 0 means the folder.
    pub file_count: usize,
    pub created_at: DateTime<Utc>,
}

async fn list(
    user: CurrentUser,
    pool: web::Data<Db>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    let client = pool.get().await?;
    let rows = client
        .query(
            "SELECT id, token, label, folder_id, permissions, password_hash,
                    expires_at, usage_limit, usage_count, allow_offline,
                    message_ids, created_at
               FROM shares
              WHERE user_id = $1 AND NOT revoked
              ORDER BY created_at DESC",
            &[&user.id],
        )
        .await?;

    let shares: Vec<ShareView> = rows
        .iter()
        .map(|row| {
            let token: String = row.get("token");
            let bits: i64 = row.get("permissions");
            let password_hash: Option<String> = row.get("password_hash");
            ShareView {
                id: row.get("id"),
                url: format!("{}/s/{}", config.public_base_url, token),
                token,
                label: row.get("label"),
                folder_id: row.get("folder_id"),
                permissions: SharePermissions::from_bits_truncate(bits).into(),
                has_password: password_hash.is_some(),
                expires_at: row.get("expires_at"),
                usage_limit: row.get("usage_limit"),
                usage_count: row.get("usage_count"),
                allow_offline: row.get("allow_offline"),
                file_count: row
                    .get::<_, Option<Vec<i32>>>("message_ids")
                    .map(|ids| ids.len())
                    .unwrap_or(0),
                created_at: row.get("created_at"),
            }
        })
        .collect();

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "shares": shares,
        "offline_available": config.offline_sharing_available(),
    })))
}

#[derive(Deserialize)]
pub struct CreateShare {
    pub label: Option<String>,
    pub folder_id: Option<Uuid>,
    pub permissions: SharePermissionView,
    pub password: Option<String>,
    pub expiry_hours: Option<i64>,
    pub usage_limit: Option<i32>,
    #[serde(default)]
    pub allow_offline: bool,
    /// Specific files to expose. Empty or absent shares the whole folder.
    #[serde(default)]
    pub message_ids: Vec<i32>,
}

async fn create(
    user: CurrentUser,
    body: web::Json<CreateShare>,
    pool: web::Data<Db>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    permissions::require(&pool, user.id, permissions::Capability::Share).await?;

    let bits: SharePermissions = body.permissions.into();
    if bits.is_empty() {
        return Err(ApiError::BadRequest(
            "A link that grants nothing is no use. Allow at least one action.".into(),
        ));
    }

    let client = pool.get().await?;

    // Offline access needs the server-wrapped session to exist, which only
    // happens if the owner opted in. Refusing here beats handing out a link
    // that silently stops working the moment they sign out.
    if body.allow_offline {
        if !config.offline_sharing_available() {
            return Err(ApiError::BadRequest(
                "Offline sharing is not configured on this server.".into(),
            ));
        }
        let ready: bool = client
            .query_one(
                "SELECT (session_srv_enc IS NOT NULL) AS ready
                   FROM tg_sessions WHERE user_id = $1",
                &[&user.id],
            )
            .await
            .map(|row| row.get("ready"))
            .unwrap_or(false);
        if !ready {
            return Err(ApiError::BadRequest(
                "Turn on \"keep links working while I am signed out\" first.".into(),
            ));
        }
    }

    // A folder must belong to the caller. Without this check a user could
    // share someone else's folder by guessing its id.
    if let Some(folder) = body.folder_id {
        let owned = client
            .query_opt(
                "SELECT 1 FROM folders WHERE id = $1 AND user_id = $2",
                &[&folder, &user.id],
            )
            .await?;
        if owned.is_none() {
            return Err(ApiError::NotFound("No such folder.".into()));
        }
    }

    let password_hash = match body.password.as_deref() {
        Some(password) if !password.is_empty() => Some(
            crate::auth::hash_password(password)
                .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?,
        ),
        _ => None,
    };

    let expires_at = body
        .expiry_hours
        .filter(|hours| *hours > 0)
        .map(|hours| Utc::now() + Duration::hours(hours));

    let token = generate_token();

    // An empty list means "the whole folder", stored as NULL so the
    // public side can tell "no restriction" from "restricted to nothing"
    // — the latter would be a link that shows no files at all.
    let message_ids: Option<&[i32]> = if body.message_ids.is_empty() {
        None
    } else {
        Some(&body.message_ids)
    };

    let row = client
        .query_one(
            "INSERT INTO shares
                (user_id, folder_id, token, permissions, password_hash,
                 expires_at, usage_limit, label, allow_offline, message_ids)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             RETURNING id, created_at",
            &[
                &user.id,
                &body.folder_id,
                &token,
                &bits.bits(),
                &password_hash,
                &expires_at,
                &body.usage_limit,
                &body.label,
                &body.allow_offline,
                &message_ids,
            ],
        )
        .await?;

    audit(
        &pool,
        Some(user.id),
        Some(user.id),
        "share.created",
        Some(serde_json::json!({
            "permissions": bits.bits(),
            "has_password": password_hash.is_some(),
            "allow_offline": body.allow_offline,
        })),
    )
    .await;

    Ok(HttpResponse::Created().json(ShareView {
        id: row.get("id"),
        url: format!("{}/s/{}", config.public_base_url, token),
        token,
        label: body.label.clone(),
        folder_id: body.folder_id,
        permissions: body.permissions,
        has_password: password_hash.is_some(),
        expires_at,
        usage_limit: body.usage_limit,
        usage_count: 0,
        allow_offline: body.allow_offline,
        file_count: body.message_ids.len(),
        created_at: row.get("created_at"),
    }))
}

async fn revoke(
    user: CurrentUser,
    path: web::Path<Uuid>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let id = path.into_inner();
    let client = pool.get().await?;

    // Marked revoked rather than deleted: the audit trail should still be
    // able to explain a link that someone remembers having.
    let affected = client
        .execute(
            "UPDATE shares SET revoked = TRUE WHERE id = $1 AND user_id = $2",
            &[&id, &user.id],
        )
        .await?;

    if affected == 0 {
        return Err(ApiError::NotFound("No such link.".into()));
    }

    audit(&pool, Some(user.id), Some(user.id), "share.revoked", None).await;
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct OfflineRequest {
    /// Turning it on requires re-typing the account password: the session
    /// has to be decrypted to be re-wrapped, and this is a deliberate
    /// weakening of the storage guarantee, so it should not be one click.
    pub password: Option<String>,
    pub enabled: bool,
}

/// Stores (or clears) a copy of the session wrapped with the server secret.
///
/// See migrations/0004 for why this exists and exactly what it gives up.
async fn enable_offline(
    request: HttpRequest,
    user: CurrentUser,
    body: web::Json<OfflineRequest>,
    pool: web::Data<Db>,
    keys: web::Data<KeyCache>,
    config: web::Data<Config>,
) -> ApiResult<HttpResponse> {
    let client = pool.get().await?;

    if !body.enabled {
        client
            .execute(
                "UPDATE tg_sessions
                    SET session_srv_enc = NULL, session_srv_nonce = NULL,
                        api_hash_srv_enc = NULL, api_hash_srv_nonce = NULL,
                        offline_enabled_at = NULL
                  WHERE user_id = $1",
                &[&user.id],
            )
            .await?;
        // Links relying on it must stop claiming they work offline.
        client
            .execute(
                "UPDATE shares SET allow_offline = FALSE WHERE user_id = $1",
                &[&user.id],
            )
            .await?;
        audit(&pool, Some(user.id), Some(user.id), "share.offline_disabled", None).await;
        return Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true, "enabled": false })));
    }

    let secret = config
        .share_secret
        .as_deref()
        .ok_or_else(|| ApiError::BadRequest("Offline sharing is not configured on this server.".into()))?;

    let password = body
        .password
        .as_deref()
        .filter(|p| !p.is_empty())
        .ok_or_else(|| ApiError::BadRequest("Confirm your password to turn this on.".into()))?;

    let stored: String = client
        .query_one("SELECT password_hash FROM users WHERE id = $1", &[&user.id])
        .await?
        .get("password_hash");
    let ok = crate::auth::verify_password(password, &stored)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    if !ok {
        return Err(ApiError::Forbidden("That password is wrong.".into()));
    }

    // Decrypt with the live per-user key, then re-wrap under the server
    // secret. Both copies are kept: ordinary use still goes through the
    // password-derived one.
    let cookie = request
        .cookie(SESSION_COOKIE)
        .ok_or(ApiError::Unauthenticated)?;
    let user_key = keys
        .get(&hash_session_token(cookie.value()))
        .ok_or_else(|| ApiError::Forbidden("Please sign in again.".into()))?;

    let row = client
        .query_opt(
            "SELECT session_enc, session_nonce, api_hash_enc, api_hash_nonce
               FROM tg_sessions WHERE user_id = $1",
            &[&user.id],
        )
        .await?
        .ok_or_else(|| ApiError::BadRequest("Connect Telegram first.".into()))?;

    let session = crypto::decrypt(
        &user_key,
        &row.get::<_, Vec<u8>>("session_enc"),
        &row.get::<_, Vec<u8>>("session_nonce"),
    )
    .map_err(|_| ApiError::Internal(anyhow::anyhow!("stored session did not decrypt")))?;
    let api_hash = crypto::decrypt(
        &user_key,
        &row.get::<_, Vec<u8>>("api_hash_enc"),
        &row.get::<_, Vec<u8>>("api_hash_nonce"),
    )
    .map_err(|_| ApiError::Internal(anyhow::anyhow!("stored api hash did not decrypt")))?;

    let server_key = crate::shares::public::server_key(secret, user.id)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

    let (session_enc, session_nonce) = crypto::encrypt(&server_key, &session)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;
    let (hash_enc, hash_nonce) = crypto::encrypt(&server_key, &api_hash)
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

    client
        .execute(
            "UPDATE tg_sessions
                SET session_srv_enc = $1, session_srv_nonce = $2,
                    api_hash_srv_enc = $3, api_hash_srv_nonce = $4,
                    offline_enabled_at = now()
              WHERE user_id = $5",
            &[&session_enc, &session_nonce, &hash_enc, &hash_nonce, &user.id],
        )
        .await?;

    audit(&pool, Some(user.id), Some(user.id), "share.offline_enabled", None).await;
    tracing::info!(user = %user.id, "offline sharing enabled");

    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true, "enabled": true })))
}
