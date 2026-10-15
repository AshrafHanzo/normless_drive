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
    let rows: Vec<(Uuid, String, String, String, DateTime<Utc>, Option<DateTime<Utc>>, bool)> =
        sqlx::query_as(
            "SELECT u.id, u.email, u.role, u.status, u.created_at, u.last_login_at,
                    (t.user_id IS NOT NULL) AS telegram_connected
             FROM users u
             LEFT JOIN tg_sessions t ON t.user_id = u.id
             ORDER BY
                 -- Pending first: the queue an admin actually has to act on.
                 CASE u.status WHEN 'pending' THEN 0 ELSE 1 END,
                 u.created_at DESC",
        )
        .fetch_all(pool.get_ref())
        .await?;

    let users: Vec<AdminUserRow> = rows
        .into_iter()
        .map(
            |(id, email, role, status, created_at, last_login_at, telegram_connected)| {
                AdminUserRow {
                    id,
                    email,
                    role,
                    status,
                    created_at,
                    last_login_at,
                    telegram_connected,
                }
            },
        )
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

    let affected = sqlx::query("UPDATE users SET status = $1 WHERE id = $2")
        .bind(status.as_str())
        .bind(target)
        .execute(pool.get_ref())
        .await?
        .rows_affected();

    if affected == 0 {
        return Err(ApiError::NotFound("No such user.".into()));
    }

    // Revoke live sessions when access is withdrawn. Without this the user
    // keeps working until their cookie expires -- though `CurrentUser` also
    // re-checks status per request, so this is belt and braces.
    if !status.may_sign_in() {
        sqlx::query("DELETE FROM app_sessions WHERE user_id = $1")
            .bind(target)
            .execute(pool.get_ref())
            .await?;
    }

    audit(
        pool.get_ref(),
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
    if role != Role::Admin {
        let remaining: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM users WHERE role = 'admin' AND status = 'active' AND id <> $1",
        )
        .bind(target)
        .fetch_one(pool.get_ref())
        .await?;
        if remaining == 0 {
            return Err(ApiError::BadRequest(
                "That is the only active administrator. Promote someone else first.".into(),
            ));
        }
    }

    let affected = sqlx::query("UPDATE users SET role = $1 WHERE id = $2")
        .bind(role.as_str())
        .bind(target)
        .execute(pool.get_ref())
        .await?
        .rows_affected();

    if affected == 0 {
        return Err(ApiError::NotFound("No such user.".into()));
    }

    audit(
        pool.get_ref(),
        Some(target),
        Some(admin.0.id),
        "admin.set_role",
        Some(serde_json::json!({ "role": role.as_str() })),
    )
    .await;

    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true, "role": role })))
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

    let rows: Vec<(i64, Option<Uuid>, Option<Uuid>, String, Option<serde_json::Value>, DateTime<Utc>)> =
        sqlx::query_as(
            "SELECT id, user_id, actor_id, action, detail, created_at
             FROM audit ORDER BY created_at DESC LIMIT $1",
        )
        .bind(limit)
        .fetch_all(pool.get_ref())
        .await?;

    let entries: Vec<serde_json::Value> = rows
        .into_iter()
        .map(|(id, user_id, actor_id, action, detail, created_at)| {
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
