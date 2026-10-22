//! Administrator routes: who exists, who may sign in, who is an admin.
//!
//! Every handler here takes `AdminUser`, so the privilege check cannot be
//! forgotten — it happens during extraction, before the body runs.

use actix_web::{web, HttpResponse};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::{extract::AdminUser, routes::audit, Role, Status},
    db::Db,
    error::{ApiError, ApiResult},
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/admin")
            .route("/users", web::get().to(list_users))
            .route("/users/{id}/status", web::post().to(set_status))
            .route("/users/{id}/role", web::post().to(set_role))
            .route("/users/{id}/permissions", web::get().to(get_permissions))
            .route("/users/{id}/permissions", web::post().to(set_permissions))
            .route("/audit", web::get().to(list_audit)),
    );
}

#[derive(Serialize)]
pub struct AdminUserRow {
    pub id: Uuid,
    pub email: String,
    pub role: String,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    /// Whether this user has connected a Telegram account yet. Useful for an
    /// admin deciding whether an account is actually in use.
    pub telegram_connected: bool,
}

async fn list_users(_admin: AdminUser, pool: web::Data<Db>) -> ApiResult<HttpResponse> {
    let client = pool.get().await?;
    let rows = client
        .query(
            "SELECT u.id, u.email, u.role, u.status, u.created_at, u.last_login_at,
                    (t.user_id IS NOT NULL) AS telegram_connected
             FROM users u
             LEFT JOIN tg_sessions t ON t.user_id = u.id
             ORDER BY
                 -- Pending first: the queue an admin actually has to act on.
                 CASE u.status WHEN 'pending' THEN 0 ELSE 1 END,
                 u.created_at DESC",
            &[],
        )
        .await?;

    let users: Vec<AdminUserRow> = rows
        .iter()
        .map(|row| AdminUserRow {
            id: row.get("id"),
            email: row.get("email"),
            role: row.get("role"),
            status: row.get("status"),
            created_at: row.get("created_at"),
            last_login_at: row.get("last_login_at"),
            telegram_connected: row.get("telegram_connected"),
        })
        .collect();

    Ok(HttpResponse::Ok().json(serde_json::json!({ "users": users })))
}

#[derive(Deserialize)]
pub struct StatusChange {
    pub status: String,
}

async fn set_status(
    admin: AdminUser,
    path: web::Path<Uuid>,
    body: web::Json<StatusChange>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let target = path.into_inner();
    let status = Status::parse(&body.status)
        .ok_or_else(|| ApiError::BadRequest(format!("Unknown status {:?}.", body.status)))?;

    // Locking yourself out is the one mistake with no in-app recovery: there
    // would be no active admin left to undo it.
    if target == admin.0.id && !status.may_sign_in() {
        return Err(ApiError::BadRequest(
            "You cannot suspend your own account.".into(),
        ));
    }

    let client = pool.get().await?;
    let affected = client
        .execute(
            "UPDATE users SET status = $1 WHERE id = $2",
            &[&status.as_str(), &target],
        )
        .await?;

    if affected == 0 {
        return Err(ApiError::NotFound("No such user.".into()));
    }

    // Revoke live sessions when access is withdrawn. Without this the user
    // keeps working until their cookie expires -- though `CurrentUser` also
    // re-checks status per request, so this is belt and braces.
    if !status.may_sign_in() {
        client
            .execute("DELETE FROM app_sessions WHERE user_id = $1", &[&target])
            .await?;
    }

    audit(
        &pool,
        Some(target),
        Some(admin.0.id),
        "admin.set_status",
        Some(serde_json::json!({ "status": status.as_str() })),
    )
    .await;

    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true, "status": status })))
}

#[derive(Deserialize)]
pub struct RoleChange {
    pub role: String,
}

async fn set_role(
    admin: AdminUser,
    path: web::Path<Uuid>,
    body: web::Json<RoleChange>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let target = path.into_inner();
    let role = Role::parse(&body.role)
        .ok_or_else(|| ApiError::BadRequest(format!("Unknown role {:?}.", body.role)))?;

    if target == admin.0.id && role != Role::Admin {
        return Err(ApiError::BadRequest(
            "You cannot remove your own administrator access.".into(),
        ));
    }

    // Demoting the last admin leaves nobody who can approve signups or
    // promote anyone, and the only fix would be editing the database by
    // hand. Refuse rather than let that happen.
    let client = pool.get().await?;
    if role != Role::Admin {
        let row = client
            .query_one(
                "SELECT count(*) FROM users
                  WHERE role = 'admin' AND status = 'active' AND id <> $1",
                &[&target],
            )
            .await?;
        let remaining: i64 = row.get(0);
        if remaining == 0 {
            return Err(ApiError::BadRequest(
                "That is the only active administrator. Promote someone else first.".into(),
            ));
        }
    }

    let affected = client
        .execute(
            "UPDATE users SET role = $1 WHERE id = $2",
            &[&role.as_str(), &target],
        )
        .await?;

    if affected == 0 {
        return Err(ApiError::NotFound("No such user.".into()));
    }

    audit(
        &pool,
        Some(target),
        Some(admin.0.id),
        "admin.set_role",
        Some(serde_json::json!({ "role": role.as_str() })),
    )
    .await;

    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true, "role": role })))
}

async fn get_permissions(
    _admin: AdminUser,
    path: web::Path<Uuid>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let target = path.into_inner();
    let limits = crate::permissions::load(&pool, target).await?;
    let used = crate::permissions::usage(&pool, target).await?;
    Ok(HttpResponse::Ok().json(serde_json::json!({
        "permissions": limits,
        "used_bytes": used,
    })))
}

#[derive(Deserialize)]
pub struct PermissionChange {
    pub can_upload: bool,
    pub can_download: bool,
    pub can_delete: bool,
    pub can_create_folders: bool,
    pub can_share: bool,
    /// Bytes. Zero means no cap.
    pub storage_quota_bytes: i64,
}

async fn set_permissions(
    admin: AdminUser,
    path: web::Path<Uuid>,
    body: web::Json<PermissionChange>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let target = path.into_inner();

    if body.storage_quota_bytes < 0 {
        return Err(ApiError::BadRequest(
            "A storage limit cannot be negative. Use 0 for no limit.".into(),
        ));
    }

    let client = pool.get().await?;
    // Upsert: an account created before this table existed has no row yet,
    // and an admin editing it should not have to care.
    let affected = client
        .execute(
            "INSERT INTO user_permissions
                (user_id, can_upload, can_download, can_delete,
                 can_create_folders, can_share, storage_quota_bytes, updated_at)
             VALUES ($1, $2, $3, $4, $5, $6, $7, now())
             ON CONFLICT (user_id) DO UPDATE SET
                can_upload = EXCLUDED.can_upload,
                can_download = EXCLUDED.can_download,
                can_delete = EXCLUDED.can_delete,
                can_create_folders = EXCLUDED.can_create_folders,
                can_share = EXCLUDED.can_share,
                storage_quota_bytes = EXCLUDED.storage_quota_bytes,
                updated_at = now()",
            &[
                &target,
                &body.can_upload,
                &body.can_download,
                &body.can_delete,
                &body.can_create_folders,
                &body.can_share,
                &body.storage_quota_bytes,
            ],
        )
        .await?;

    if affected == 0 {
        return Err(ApiError::NotFound("No such user.".into()));
    }

    audit(
        &pool,
        Some(target),
        Some(admin.0.id),
        "admin.set_permissions",
        Some(serde_json::json!({
            "can_upload": body.can_upload,
            "can_download": body.can_download,
            "can_delete": body.can_delete,
            "can_create_folders": body.can_create_folders,
            "can_share": body.can_share,
            "storage_quota_bytes": body.storage_quota_bytes,
        })),
    )
    .await;

    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct AuditQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

async fn list_audit(
    _admin: AdminUser,
    query: web::Query<AuditQuery>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let limit = query.limit.unwrap_or(100).clamp(1, 500);

    let client = pool.get().await?;
    let rows = client
        .query(
            "SELECT id, user_id, actor_id, action, detail, created_at
             FROM audit ORDER BY created_at DESC LIMIT $1",
            &[&limit],
        )
        .await?;

    let entries: Vec<serde_json::Value> = rows
        .iter()
        .map(|row| {
            let id: i64 = row.get("id");
            let user_id: Option<Uuid> = row.get("user_id");
            let actor_id: Option<Uuid> = row.get("actor_id");
            let action: String = row.get("action");
            let detail: Option<serde_json::Value> = row.get("detail");
            let created_at: DateTime<Utc> = row.get("created_at");
            serde_json::json!({
                "id": id,
                "user_id": user_id,
                "actor_id": actor_id,
                "action": action,
                "detail": detail,
                "created_at": created_at,
            })
        })
        .collect();

    Ok(HttpResponse::Ok().json(serde_json::json!({ "entries": entries })))
}
