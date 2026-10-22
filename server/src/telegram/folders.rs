//! Folders, which are private Telegram channels.
//!
//! The desktop app uses the same scheme: one channel per folder, named
//! `<folder> [TD]`, so a drive created here is recognisable from the
//! Telegram app itself and from the desktop client.
//!
//! A channel is addressed by `(id, access_hash)`, and the access hash is
//! issued per account — so it is stored with the folder rather than
//! re-resolved on every request, which would cost a round trip each time.

use actix_web::{web, HttpRequest, HttpResponse};
use grammers_client::types::Peer;
use grammers_client::Client;
use grammers_tl_types as tl;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{connect::SharedClients, TelegramError};
use crate::{
    auth::CurrentUser,
    db::Db,
    error::{ApiError, ApiResult},
    keycache::KeyCache,
    permissions,
};

/// Marks a channel as belonging to a Telegram Drive, matching the desktop
/// app's convention so the two can see each other's folders.
const FOLDER_SUFFIX: &str = " [TD]";
const FOLDER_ABOUT: &str = "Telegram Drive Storage Folder\n[telegram-drive-folder]";

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/folders")
            .route("", web::get().to(list))
            .route("", web::post().to(create)),
    );
}

#[derive(Serialize)]
pub struct FolderRow {
    pub id: Uuid,
    pub channel_id: i64,
    pub name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

async fn list(user: CurrentUser, pool: web::Data<Db>) -> ApiResult<HttpResponse> {
    let client = pool.get().await?;
    let rows = client
        .query(
            "SELECT id, channel_id, name, created_at
               FROM folders WHERE user_id = $1
              ORDER BY display_order, created_at",
            &[&user.id],
        )
        .await?;

    let folders: Vec<FolderRow> = rows
        .iter()
        .map(|row| FolderRow {
            id: row.get("id"),
            channel_id: row.get("channel_id"),
            name: row.get("name"),
            created_at: row.get("created_at"),
        })
        .collect();

    Ok(HttpResponse::Ok().json(serde_json::json!({ "folders": folders })))
}

#[derive(Deserialize)]
pub struct CreateFolder {
    pub name: String,
}

async fn create(
    request: HttpRequest,
    user: CurrentUser,
    body: web::Json<CreateFolder>,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    permissions::require(&pool, user.id, permissions::Capability::CreateFolders).await?;

    let name = body.name.trim();
    if name.is_empty() {
        return Err(ApiError::BadRequest("Give the folder a name.".into()));
    }
    // Telegram caps a channel title at 128 characters, and the suffix has
    // to fit inside that too.
    if name.chars().count() > 100 {
        return Err(ApiError::BadRequest(
            "That name is too long — 100 characters at most.".into(),
        ));
    }

    let connection = super::files::connection_for(&request, &user, &keys, &clients, &pool).await?;
    let (channel_id, access_hash) =
        create_channel(&connection.client, &format!("{name}{FOLDER_SUFFIX}")).await?;

    let client = pool.get().await?;
    let row = client
        .query_one(
            "INSERT INTO folders (user_id, channel_id, name, access_hash, display_order)
             VALUES ($1, $2, $3, $4,
                     COALESCE((SELECT max(display_order) + 1 FROM folders WHERE user_id = $1), 0))
             ON CONFLICT (user_id, channel_id) DO UPDATE SET name = EXCLUDED.name
             RETURNING id, created_at",
            &[&user.id, &channel_id, &name, &access_hash],
        )
        .await?;

    tracing::info!(user = %user.id, %channel_id, "folder created");

    Ok(HttpResponse::Created().json(FolderRow {
        id: row.get("id"),
        channel_id,
        name: name.to_string(),
        created_at: row.get("created_at"),
    }))
}

/// Creates a private broadcast channel and returns its id and access hash.
async fn create_channel(client: &Client, title: &str) -> Result<(i64, i64), TelegramError> {
    let updates = client
        .invoke(&tl::functions::channels::CreateChannel {
            broadcast: true,
            megagroup: false,
            for_import: false,
            forum: false,
            title: title.to_string(),
            about: FOLDER_ABOUT.to_string(),
            geo_point: None,
            address: None,
            ttl_period: None,
        })
        .await
        .map_err(|error| {
            TelegramError::Other(format!("Telegram refused to create the folder: {error}"))
        })?;

    // CreateChannel answers with an Updates envelope; the channel itself is
    // in its chats list rather than being returned directly.
    let chats = match updates {
        tl::enums::Updates::Updates(u) => u.chats,
        tl::enums::Updates::Combined(u) => u.chats,
        _ => Vec::new(),
    };

    chats
        .into_iter()
        .find_map(|chat| match chat {
            tl::enums::Chat::Channel(channel) => Some((channel.id, channel.access_hash?)),
            _ => None,
        })
        .ok_or_else(|| {
            TelegramError::Other("Telegram did not return the new folder".to_string())
        })
}

/// Resolves a stored folder into a peer usable for file operations.
///
/// `None` means Saved Messages, which is the user's own chat and needs no
/// folders row.
pub async fn resolve(
    client: &Client,
    pool: &Db,
    user: Uuid,
    folder: Option<Uuid>,
) -> ApiResult<Peer> {
    let Some(folder_id) = folder else {
        let me = client.get_me().await.map_err(|error| {
            TelegramError::Other(format!("could not identify the account: {error}"))
        })?;
        return Ok(Peer::User(me));
    };

    let db = pool.get().await?;
    // Scoped to the caller: without `user_id` in the WHERE clause any user
    // could address another user's folder by guessing its id.
    let row = db
        .query_opt(
            "SELECT channel_id, access_hash FROM folders WHERE id = $1 AND user_id = $2",
            &[&folder_id, &user],
        )
        .await?
        .ok_or_else(|| ApiError::NotFound("No such folder.".into()))?;

    let channel_id: i64 = row.get("channel_id");
    let access_hash: Option<i64> = row.get("access_hash");
    let access_hash = access_hash.ok_or_else(|| {
        ApiError::Internal(anyhow::anyhow!(
            "folder {folder_id} has no access hash and cannot be addressed"
        ))
    })?;

    // Ask Telegram for the channel rather than constructing the TL struct
    // by hand. `tl::types::Channel` gains fields with every TL layer, so a
    // hand-built one stops compiling on each grammers bump and carries
    // invented values for everything we do not know.
    let result = client
        .invoke(&tl::functions::channels::GetChannels {
            id: vec![tl::enums::InputChannel::Channel(tl::types::InputChannel {
                channel_id,
                access_hash,
            })],
        })
        .await
        .map_err(|error| {
            TelegramError::Other(format!("could not open the folder's channel: {error}"))
        })?;

    let chats = match result {
        tl::enums::messages::Chats::Chats(c) => c.chats,
        tl::enums::messages::Chats::Slice(c) => c.chats,
    };

    let chat = chats
        .into_iter()
        .find(|chat| matches!(chat, tl::enums::Chat::Channel(_)))
        .ok_or_else(|| {
            ApiError::NotFound(
                "That folder no longer exists in Telegram. It may have been deleted there.".into(),
            )
        })?;

    Ok(Peer::Channel(grammers_client::types::Channel::from_raw(
        chat,
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_suffix_matches_the_desktop_app() {
        // Changing this silently would split a user's drive in two: the
        // desktop client would stop recognising folders made here.
        assert_eq!(FOLDER_SUFFIX, " [TD]");
        assert!(FOLDER_ABOUT.contains("[telegram-drive-folder]"));
    }
}
