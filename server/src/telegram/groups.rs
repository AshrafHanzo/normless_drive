//! Folder groups — the sidebar organiser from the desktop app.
//!
//! A group holds folders, not files, and lives only in this database.
//! Nothing here touches Telegram, which is the whole distinction: a
//! folder is a real channel that syncs everywhere, a group is a label on
//! it that does not.

use actix_web::{web, HttpResponse};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    auth::CurrentUser,
    db::Db,
    error::{ApiError, ApiResult},
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/groups")
            .route("", web::get().to(list))
            .route("", web::post().to(create))
            .route("/{id}", web::delete().to(remove))
            .route("/assign", web::post().to(assign)),
    );
}

#[derive(Serialize)]
pub struct Group {
    pub id: Uuid,
    pub name: String,
    pub color_hex: String,
    pub display_order: i32,
}

async fn list(user: CurrentUser, pool: web::Data<Db>) -> ApiResult<HttpResponse> {
    let client = pool.get().await?;
    let rows = client
        .query(
            "SELECT id, name, color_hex, display_order
               FROM groups WHERE user_id = $1
              ORDER BY display_order, created_at",
            &[&user.id],
        )
        .await?;

    let groups: Vec<Group> = rows
        .iter()
        .map(|row| Group {
            id: row.get("id"),
            name: row.get("name"),
            color_hex: row.get("color_hex"),
            display_order: row.get("display_order"),
        })
        .collect();

    Ok(HttpResponse::Ok().json(serde_json::json!({ "groups": groups })))
}

#[derive(Deserialize)]
pub struct CreateGroup {
    pub name: String,
    #[serde(default)]
    pub color_hex: Option<String>,
}

async fn create(
    user: CurrentUser,
    body: web::Json<CreateGroup>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("Give the group a name.".into()));
    }
    if name.chars().count() > 60 {
        return Err(ApiError::BadRequest(
            "That name is too long — 60 characters at most.".into(),
        ));
    }

    let color = body.color_hex.as_deref().unwrap_or("#2d7ff9");
    // The colour is interpolated into a style attribute in the browser, so
    // anything that is not a plain hex triple has no business being here.
    if !is_hex_colour(color) {
        return Err(ApiError::BadRequest("That is not a valid colour.".into()));
    }

    let client = pool.get().await?;
    let row = client
        .query_one(
            "INSERT INTO groups (user_id, name, color_hex, display_order)
             VALUES ($1, $2, $3,
                     COALESCE((SELECT max(display_order) + 1 FROM groups WHERE user_id = $1), 0))
             RETURNING id, display_order",
            &[&user.id, &name, &color],
        )
        .await?;

    Ok(HttpResponse::Created().json(Group {
        id: row.get("id"),
        name: name.to_string(),
        color_hex: color.to_string(),
        display_order: row.get("display_order"),
    }))
}

async fn remove(
    user: CurrentUser,
    path: web::Path<Uuid>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let id = path.into_inner();
    let client = pool.get().await?;

    // The foreign key is ON DELETE SET NULL, so folders survive and simply
    // become ungrouped. Deleting a group must never touch a channel.
    let affected = client
        .execute(
            "DELETE FROM groups WHERE id = $1 AND user_id = $2",
            &[&id, &user.id],
        )
        .await?;

    if affected == 0 {
        return Err(ApiError::NotFound("No such group.".into()));
    }
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

#[derive(Deserialize)]
pub struct Assign {
    pub folder_id: Uuid,
    /// `None` removes the folder from whatever group it is in.
    #[serde(default)]
    pub group_id: Option<Uuid>,
}

async fn assign(
    user: CurrentUser,
    body: web::Json<Assign>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let client = pool.get().await?;

    // Both sides scoped to the caller. Without this a user could move
    // someone else's folder, or into someone else's group, by guessing ids.
    if let Some(group) = body.group_id {
        let owned = client
            .query_opt(
                "SELECT 1 FROM groups WHERE id = $1 AND user_id = $2",
                &[&group, &user.id],
            )
            .await?;
        if owned.is_none() {
            return Err(ApiError::NotFound("No such group.".into()));
        }
    }

    let affected = client
        .execute(
            "UPDATE folders SET group_id = $1 WHERE id = $2 AND user_id = $3",
            &[&body.group_id, &body.folder_id, &user.id],
        )
        .await?;

    if affected == 0 {
        return Err(ApiError::NotFound("No such folder.".into()));
    }
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

fn is_hex_colour(value: &str) -> bool {
    let Some(digits) = value.strip_prefix('#') else {
        return false;
    };
    matches!(digits.len(), 3 | 6) && digits.chars().all(|c| c.is_ascii_hexdigit())
}

#[cfg(test)]
mod tests {
    use super::is_hex_colour;

    #[test]
    fn real_colours_are_accepted() {
        for good in ["#fff", "#2d7ff9", "#ABCDEF", "#000000"] {
            assert!(is_hex_colour(good), "should accept {good}");
        }
    }

    #[test]
    fn anything_that_could_escape_a_style_attribute_is_refused() {
        // This value ends up inside a style attribute, so a stray quote or
        // semicolon would let a group name smuggle in CSS.
        for bad in [
            "red",
            "#12",
            "#1234567",
            "#xyzxyz",
            "",
            "#fff;background:url(x)",
            "\"><script>",
        ] {
            assert!(!is_hex_colour(bad), "should refuse {bad:?}");
        }
    }
}
