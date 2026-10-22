//! Listing, downloading and uploading files in the user's Telegram.
//!
//! "Saved Messages" is the user's own chat with themselves, which is what
//! this treats as the drive root. Each file is one message carrying a
//! document.
//!
//! Nothing is buffered to disk in either direction: a download streams from
//! Telegram straight into the HTTP response, and an upload streams from the
//! request body straight into Telegram.

use actix_multipart::Multipart;
use actix_web::{web, HttpRequest, HttpResponse};
use futures::StreamExt;
use grammers_client::types::Media;
use grammers_client::{Client, InputMessage};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{connect::SharedClients, Connection, TelegramError};
use crate::{
    auth::{hash_session_token, CurrentUser, SESSION_COOKIE},
    db::Db,
    error::{ApiError, ApiResult},
    keycache::KeyCache,
    permissions,
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/files")
            .route("", web::get().to(list))
            .route("", web::post().to(upload))
            .route("/{id}/download", web::get().to(download))
            .route("/{id}", web::delete().to(remove)),
    );
}

#[derive(Serialize)]
pub struct FileRow {
    pub id: i32,
    pub name: String,
    pub size: i64,
    pub mime_type: Option<String>,
    pub date: i64,
}

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub limit: Option<usize>,
    /// Which folder to read. Absent means Saved Messages.
    #[serde(default)]
    pub folder: Option<Uuid>,
}

/// Download and upload take the folder in the query rather than a list
/// query, so they share this instead of `ListQuery`.
#[derive(Deserialize)]
pub struct FolderQuery {
    #[serde(default)]
    pub folder: Option<Uuid>,
    /// Upload only. The browser knows the file's length and Telegram needs
    /// it up front, so it travels here rather than being discovered by
    /// reading the whole body into memory first.
    #[serde(default)]
    pub size: Option<u64>,
}

/// Pulls the connection for the caller, translating the two "you need to
/// sign in again" cases into something the UI can act on.
pub async fn connection_for(
    request: &HttpRequest,
    user: &CurrentUser,
    keys: &KeyCache,
    clients: &SharedClients,
    pool: &Db,
) -> ApiResult<std::sync::Arc<Connection>> {
    let cookie = request
        .cookie(SESSION_COOKIE)
        .ok_or(ApiError::Unauthenticated)?;
    let key = keys
        .get(&hash_session_token(cookie.value()))
        .ok_or_else(|| ApiError::Forbidden(TelegramError::Locked.to_string()))?;

    clients
        .get(user.id, &key, pool)
        .await
        .map_err(ApiError::from)
}

async fn list(
    request: HttpRequest,
    user: CurrentUser,
    query: web::Query<ListQuery>,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let connection = connection_for(&request, &user, &keys, &clients, &pool).await?;
    let peer = super::folders::resolve(&connection.client, &pool, user.id, query.folder).await?;

    // Capped: Saved Messages can hold years of history, and walking all of
    // it on every page load would be slow and pointless.
    let limit = query.limit.unwrap_or(100).clamp(1, 500);

    let mut messages = connection.client.iter_messages(&peer);
    let mut files = Vec::new();

    while let Some(message) = messages
        .next()
        .await
        .map_err(|error| TelegramError::Other(format!("could not read messages: {error}")))?
    {
        // Only messages carrying a document are files; the rest are notes
        // the user sent themselves and are not part of the drive.
        let Some(Media::Document(document)) = message.media() else {
            continue;
        };

        files.push(FileRow {
            id: message.id(),
            name: non_empty(document.name(), "untitled"),
            size: document.size(),
            mime_type: document.mime_type().map(str::to_string),
            date: message.date().timestamp(),
        });

        if files.len() >= limit {
            break;
        }
    }

    Ok(HttpResponse::Ok().json(serde_json::json!({ "files": files })))
}

async fn download(
    request: HttpRequest,
    user: CurrentUser,
    path: web::Path<i32>,
    query: web::Query<FolderQuery>,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    permissions::require(&pool, user.id, permissions::Capability::Download).await?;

    let message_id = path.into_inner();
    let connection = connection_for(&request, &user, &keys, &clients, &pool).await?;
    let peer = super::folders::resolve(&connection.client, &pool, user.id, query.folder).await?;

    let messages = connection
        .client
        .get_messages_by_id(&peer, &[message_id])
        .await
        .map_err(|error| TelegramError::Other(format!("could not fetch the file: {error}")))?;

    let message = messages
        .into_iter()
        .flatten()
        .next()
        .ok_or_else(|| ApiError::NotFound("No such file.".into()))?;

    let Some(Media::Document(document)) = message.media() else {
        return Err(ApiError::NotFound("That message has no file.".into()));
    };

    let name = non_empty(document.name(), "download");
    let mime = document
        .mime_type()
        .unwrap_or("application/octet-stream")
        .to_string();
    let size = document.size();

    // Streamed, not collected: a 2 GB file must not be assembled in memory
    // before the first byte reaches the browser.
    let media = Media::Document(document);
    let client = connection.client.clone();
    let stream = async_stream::stream! {
        let mut chunks = client.iter_download(&media);
        loop {
            match chunks.next().await {
                Ok(Some(chunk)) => yield Ok::<_, actix_web::Error>(web::Bytes::from(chunk)),
                Ok(None) => break,
                Err(error) => {
                    // The response headers are already sent by now, so there
                    // is no way to turn this into a clean HTTP error — the
                    // transfer just ends short. Log it so the truncation is
                    // explainable.
                    tracing::warn!(?error, message_id, "download stream failed mid-transfer");
                    break;
                }
            }
        }
    };

    Ok(HttpResponse::Ok()
        .content_type(mime)
        .insert_header((
            actix_web::http::header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{}\"", sanitize_name(&name)),
        ))
        .insert_header((actix_web::http::header::CONTENT_LENGTH, size.to_string()))
        .streaming(Box::pin(stream)))
}

async fn upload(
    request: HttpRequest,
    user: CurrentUser,
    query: web::Query<FolderQuery>,
    mut payload: Multipart,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    permissions::require(&pool, user.id, permissions::Capability::Upload).await?;

    let connection = connection_for(&request, &user, &keys, &clients, &pool).await?;
    let peer = super::folders::resolve(&connection.client, &pool, user.id, query.folder).await?;

    let Some(field) = payload
        .next()
        .await
        .transpose()
        .map_err(|error| ApiError::BadRequest(format!("Malformed upload: {error}")))?
    else {
        return Err(ApiError::BadRequest("No file in the request.".into()));
    };

    let name = field
        .content_disposition()
        .and_then(|cd| cd.get_filename())
        .map(sanitize_name)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "upload".to_string());

    // grammers needs the length before it starts, and multipart carries no
    // per-part size — so the browser sends it as `?size=`, since it is the
    // one party that already knows. Nothing is buffered: the request body
    // is adapted to an AsyncRead and handed straight to Telegram, so a
    // 2 GB upload uses a chunk of memory, not 2 GB of it.
    let size = query.size.ok_or_else(|| {
        ApiError::BadRequest("The upload is missing its size.".into())
    })?;

    if size == 0 {
        return Err(ApiError::BadRequest("That file is empty.".into()));
    }
    // Telegram refuses a single document above this, and the splitter that
    // would work around it is not built yet — so say so plainly rather
    // than let the upload run for an hour and then fail.
    const TELEGRAM_MAX: u64 = 2_000_000_000;
    if size > TELEGRAM_MAX {
        return Err(ApiError::BadRequest(format!(
            "Telegram accepts at most 2 GB in one file. This is {:.2} GB.",
            size as f64 / 1_000_000_000.0
        )));
    }

    // Checked against the declared size before a byte is transferred. A
    // client that under-declares gets caught below when the stream runs
    // long, and grammers rejects a mismatched length anyway.
    permissions::check_quota(&pool, user.id, size as i64).await?;

    let body = field.map(|chunk| {
        chunk.map_err(|error| std::io::Error::other(format!("upload interrupted: {error}")))
    });
    let mut reader = tokio_util::io::StreamReader::new(body);

    let uploaded = connection
        .client
        .upload_stream(&mut reader, size as usize, name.clone())
        .await
        .map_err(|error| TelegramError::Other(format!("upload to Telegram failed: {error}")))?;

    // `text`/`document` are builder methods on an existing InputMessage,
    // not constructors — hence building from an empty caption rather than
    // calling InputMessage::text directly.
    let message = connection
        .client
        .send_message(&peer, InputMessage::from("").document(uploaded))
        .await
        .map_err(|error| TelegramError::Other(format!("could not attach the file: {error}")))?;

    // Recorded for the quota. A failure here must not fail an upload that
    // already succeeded — the file is in Telegram either way, and the
    // worst outcome is that it is not counted.
    if let Ok(db) = pool.get().await {
        let recorded = db
            .execute(
                "INSERT INTO uploads (user_id, folder_id, message_id, name, size_bytes)
                 VALUES ($1, $2, $3, $4, $5)",
                &[
                    &user.id,
                    &query.folder,
                    &message.id(),
                    &name,
                    &(size as i64),
                ],
            )
            .await;
        if let Err(error) = recorded {
            tracing::warn!(?error, "upload succeeded but was not recorded for the quota");
        }
    }

    tracing::info!(user = %user.id, name = %name, size, "uploaded");

    Ok(HttpResponse::Created().json(serde_json::json!({
        "ok": true,
        "name": name,
        "size": size
    })))
}

/// Deletes a file from Telegram.
///
/// Irreversible: this removes the message for everyone, and Telegram has
/// no undo. The confirmation lives in the UI because the API should stay
/// callable by scripts that have already decided.
async fn remove(
    request: HttpRequest,
    user: CurrentUser,
    path: web::Path<i32>,
    query: web::Query<FolderQuery>,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    permissions::require(&pool, user.id, permissions::Capability::Delete).await?;

    let message_id = path.into_inner();
    let connection = connection_for(&request, &user, &keys, &clients, &pool).await?;
    let peer = super::folders::resolve(&connection.client, &pool, user.id, query.folder).await?;

    connection
        .client
        .delete_messages(&peer, &[message_id])
        .await
        .map_err(|error| TelegramError::Other(format!("could not delete: {error}")))?;

    // Keep the quota tally honest. Best effort: the file is already gone
    // from Telegram, so failing the request now would be misleading.
    if let Ok(db) = pool.get().await {
        let _ = db
            .execute(
                "DELETE FROM uploads WHERE user_id = $1 AND message_id = $2",
                &[&user.id, &message_id],
            )
            .await;
    }

    tracing::info!(user = %user.id, message_id, "deleted");
    Ok(HttpResponse::Ok().json(serde_json::json!({ "ok": true })))
}

/// `Document::name()` returns an empty string rather than `None` when a
/// document has no filename, so an "is it missing" check needs doing by
/// hand in both the listing and the download path.
fn non_empty(value: &str, fallback: &str) -> String {
    if value.trim().is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

/// Strips path separators and quotes from a filename.
///
/// It goes into a `Content-Disposition` header and is shown in the UI, so a
/// name containing a quote or a newline could otherwise forge headers or
/// escape the attribute.
pub fn sanitize_name(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '/' | '\\' | '"' | '\r' | '\n' | '\0'))
        .take(255)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::sanitize_name;

    #[test]
    fn a_filename_cannot_escape_the_header_or_the_path() {
        assert_eq!(sanitize_name("../../etc/passwd"), "....etcpasswd");
        assert_eq!(sanitize_name("re\"port.pdf"), "report.pdf");
        assert_eq!(sanitize_name("a\r\nX-Evil: yes"), "aX-Evil: yes");
    }

    #[test]
    fn an_ordinary_filename_is_untouched() {
        assert_eq!(sanitize_name("Quarterly Report 2026.pdf"), "Quarterly Report 2026.pdf");
    }

    #[test]
    fn an_absurdly_long_name_is_truncated() {
        assert_eq!(sanitize_name(&"a".repeat(400)).len(), 255);
    }
}
