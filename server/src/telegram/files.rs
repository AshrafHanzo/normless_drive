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
use grammers_client::types::{Media, Peer};
use grammers_client::{Client, InputMessage};
use serde::{Deserialize, Serialize};

use super::{connect::SharedClients, Connection, TelegramError};
use crate::{
    auth::{hash_session_token, CurrentUser, SESSION_COOKIE},
    db::Db,
    error::{ApiError, ApiResult},
    keycache::KeyCache,
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(
        web::scope("/api/files")
            .route("", web::get().to(list))
            .route("", web::post().to(upload))
            .route("/{id}/download", web::get().to(download)),
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
}

/// Resolves the user's own chat, which is what Telegram calls Saved Messages.
async fn saved_messages(client: &Client) -> Result<Peer, TelegramError> {
    let me = client
        .get_me()
        .await
        .map_err(|error| TelegramError::Other(format!("could not identify the account: {error}")))?;
    Ok(Peer::User(me))
}

/// Pulls the connection for the caller, translating the two "you need to
/// sign in again" cases into something the UI can act on.
async fn connection(
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
    let connection = connection(&request, &user, &keys, &clients, &pool).await?;
    let peer = saved_messages(&connection.client).await?;

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
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let message_id = path.into_inner();
    let connection = connection(&request, &user, &keys, &clients, &pool).await?;
    let peer = saved_messages(&connection.client).await?;

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
            format!("attachment; filename=\"{}\"", sanitize(&name)),
        ))
        .insert_header((actix_web::http::header::CONTENT_LENGTH, size.to_string()))
        .streaming(Box::pin(stream)))
}

async fn upload(
    request: HttpRequest,
    user: CurrentUser,
    mut payload: Multipart,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    let connection = connection(&request, &user, &keys, &clients, &pool).await?;
    let peer = saved_messages(&connection.client).await?;

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
        .map(sanitize)
        .filter(|n| !n.is_empty())
        .unwrap_or_else(|| "upload".to_string());

    // grammers needs the length up front, and multipart does not carry a
    // per-part size, so this reads the part into memory. That is the known
    // limitation of this first cut: fine for documents and photos, not for
    // multi-gigabyte files. Streaming needs the chunked endpoint the
    // desktop app uses, which is the next piece of work.
    let mut bytes = web::BytesMut::new();
    let mut field = field;
    while let Some(chunk) = field.next().await {
        let chunk = chunk.map_err(|error| ApiError::BadRequest(format!("Upload failed: {error}")))?;
        bytes.extend_from_slice(&chunk);
        if bytes.len() > 256 * 1024 * 1024 {
            return Err(ApiError::BadRequest(
                "This first version accepts files up to 256 MB. Larger uploads need the chunked endpoint.".into(),
            ));
        }
    }

    if bytes.is_empty() {
        return Err(ApiError::BadRequest("That file is empty.".into()));
    }

    let size = bytes.len();
    let mut cursor = std::io::Cursor::new(bytes.freeze());
    let uploaded = connection
        .client
        .upload_stream(&mut cursor, size, name.clone())
        .await
        .map_err(|error| TelegramError::Other(format!("upload to Telegram failed: {error}")))?;

    // `text`/`document` are builder methods on an existing InputMessage,
    // not constructors — hence building from an empty caption rather than
    // calling InputMessage::text directly.
    connection
        .client
        .send_message(&peer, InputMessage::from("").document(uploaded))
        .await
        .map_err(|error| TelegramError::Other(format!("could not attach the file: {error}")))?;

    tracing::info!(user = %user.id, name = %name, size, "uploaded");

    Ok(HttpResponse::Created().json(serde_json::json!({
        "ok": true,
        "name": name,
        "size": size
    })))
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
fn sanitize(name: &str) -> String {
    name.chars()
        .filter(|c| !matches!(c, '/' | '\\' | '"' | '\r' | '\n' | '\0'))
        .take(255)
        .collect::<String>()
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn a_filename_cannot_escape_the_header_or_the_path() {
        assert_eq!(sanitize("../../etc/passwd"), "....etcpasswd");
        assert_eq!(sanitize("re\"port.pdf"), "report.pdf");
        assert_eq!(sanitize("a\r\nX-Evil: yes"), "aX-Evil: yes");
    }

    #[test]
    fn an_ordinary_filename_is_untouched() {
        assert_eq!(sanitize("Quarterly Report 2026.pdf"), "Quarterly Report 2026.pdf");
    }

    #[test]
    fn an_absurdly_long_name_is_truncated() {
        assert_eq!(sanitize(&"a".repeat(400)).len(), 255);
    }
}
