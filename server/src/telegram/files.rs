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
use grammers_client::InputMessage;
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
            // Both of these must be declared before `/{id}`, and inside
            // this scope rather than beside it — a sibling resource under
            // the same prefix never gets a chance to match.
            .route("/remote", web::post().to(super::remote::remote_upload))
            .route("/move", web::post().to(move_files))
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
        .ok_or(ApiError::SessionLocked)?;

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
        match message.media() {
            // A document is a file — unless it is one slice of a large one,
            // in which case the manifest below stands for the whole thing
            // and showing the slices too would be noise.
            Some(Media::Document(document)) => {
                if super::split::is_part(message.text()) {
                    continue;
                }
                files.push(FileRow {
                    id: message.id(),
                    name: non_empty(document.name(), "untitled"),
                    size: document.size(),
                    mime_type: document.mime_type().map(str::to_string),
                    date: message.date().timestamp(),
                });
            }
            // A message with no document is a note the user sent
            // themselves and not part of the drive — unless it is the
            // manifest of a file too big for one message, which is the one
            // row that should appear for it.
            _ => {
                let Some(manifest) = super::split::parse_manifest(message.text()) else {
                    continue;
                };
                files.push(FileRow {
                    id: message.id(),
                    name: manifest.name,
                    size: manifest.size as i64,
                    mime_type: None,
                    date: message.date().timestamp(),
                });
            }
        }

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

    // Either one document, or the manifest of a file stored as several.
    // Both become a list of sources that `serve` streams as one file.
    let (sources, name, mime, size) = match message.media() {
        Some(Media::Document(document)) => {
            let name = non_empty(document.name(), "download");
            let mime = document
                .mime_type()
                .unwrap_or("application/octet-stream")
                .to_string();
            let size = document.size();
            (super::ranged::Source::single(Media::Document(document), size), name, mime, size)
        }
        _ => {
            let manifest = super::split::parse_manifest(message.text())
                .ok_or_else(|| ApiError::NotFound("That message has no file.".into()))?;
            let sources =
                super::split::resolve_parts(&connection.client, &peer, &manifest).await?;
            (
                sources,
                manifest.name,
                "application/octet-stream".to_string(),
                manifest.size as i64,
            )
        }
    };

    // Streamed, not collected: a 2 GB file must not be assembled in memory
    // before the first byte reaches the browser. `serve` also answers a
    // `Range` request, which is how the archive viewer reads a zip's index
    // without pulling the archive down.
    Ok(super::ranged::serve(
        &request,
        connection.client.clone(),
        sources,
        size,
        mime,
        format!("attachment; filename=\"{}\"", sanitize_name(&name)),
        message_id,
    ))
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

    // Checked against the declared size before a byte is transferred. A
    // client that under-declares gets caught below when the stream runs
    // long, and grammers rejects a mismatched length anyway.
    permissions::check_quota(&pool, user.id, size as i64).await?;

    let body = field.map(|chunk| {
        chunk.map_err(|error| std::io::Error::other(format!("upload interrupted: {error}")))
    });
    let reader = tokio_util::io::StreamReader::new(body);

    let message = if super::split::should_split(size) {
        send_in_parts(&connection, &peer, reader, &name, size).await?
    } else {
        let mut reader = reader;
        let uploaded = connection
            .client
            .upload_stream(&mut reader, size as usize, name.clone())
            .await
            .map_err(|error| TelegramError::Other(format!("upload to Telegram failed: {error}")))?;

        // `text`/`document` are builder methods on an existing InputMessage,
        // not constructors — hence building from an empty caption rather than
        // calling InputMessage::text directly.
        connection
            .client
            .send_message(&peer, InputMessage::from("").document(uploaded))
            .await
            .map_err(|error| TelegramError::Other(format!("could not attach the file: {error}")))?
    };

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

/// Uploads a file too big for one Telegram message, as several.
///
/// The body is read once, straight through: each part takes its slice off
/// the same stream, so a 20 GB upload needs no disk and no more memory
/// than a single one. A SHA-256 of the whole file falls out of that one
/// pass and goes in the manifest, which is what makes a reassembled
/// download checkable against what was sent.
///
/// The manifest goes last, once every part has landed. A failure part way
/// through therefore leaves orphaned parts — invisible, and costing
/// storage until they are cleared — rather than a manifest pointing at
/// messages that are not there.
async fn send_in_parts<R>(
    connection: &std::sync::Arc<Connection>,
    peer: &grammers_client::types::Peer,
    reader: R,
    name: &str,
    size: u64,
) -> ApiResult<grammers_client::types::Message>
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;

    let ranges = super::split::part_ranges(size);
    let total = ranges.len();
    let mut digest = Digesting {
        inner: reader,
        hasher: <sha2::Sha256 as sha2::Digest>::new(),
    };
    let mut orphans = Orphans {
        connection: std::sync::Arc::clone(connection),
        peer: peer.clone(),
        ids: Vec::with_capacity(total),
    };

    for (index, (_, len)) in ranges.iter().enumerate() {
        let mut slice = (&mut digest).take(*len);
        let uploaded = connection
            .client
            .upload_stream(&mut slice, *len as usize, super::split::part_name(index))
            .await
            .map_err(|error| {
                TelegramError::Other(format!(
                    "upload failed on part {} of {}: {error}",
                    index + 1,
                    total
                ))
            })?;

        let caption = super::split::part_caption(index, total, name);
        let message = connection
            .client
            .send_message(peer, InputMessage::from(caption.as_str()).document(uploaded))
            .await
            .map_err(|error| {
                TelegramError::Other(format!(
                    "could not attach part {} of {}: {error}",
                    index + 1,
                    total
                ))
            })?;
        orphans.ids.push(message.id());
    }

    let manifest = super::split::Manifest {
        schema_version: 1,
        name: name.to_string(),
        size,
        part_count: total as u32,
        part_ids: orphans.ids.clone(),
        sha256: Some(hex::encode(sha2::Digest::finalize(digest.hasher))),
    };

    let written = connection
        .client
        .send_message(
            peer,
            InputMessage::from(super::split::manifest_text(&manifest)?.as_str()),
        )
        .await
        .map_err(|error| {
            TelegramError::Other(format!(
                "all {total} parts were uploaded, but the index describing them could not be \
                 sent: {error}"
            ))
        })?;

    // The index exists, so the parts are no longer orphans.
    orphans.keep();
    Ok(written)
}

/// Removes the parts of a split upload that never got an index.
///
/// A browser that goes away mid-upload takes the request with it, and
/// actix drops the handler exactly where it stood — no error is returned,
/// no code after the await runs. `Drop` is the one thing that still
/// happens, so the cleanup lives here. Without it, stopping a 10 GB
/// upload halfway would leave several 2 GB messages in the account:
/// invisible in the drive, since nothing indexes them, and charged for
/// all the same.
struct Orphans {
    connection: std::sync::Arc<Connection>,
    peer: grammers_client::types::Peer,
    ids: Vec<i32>,
}

impl Orphans {
    /// Called once the index naming these parts has been sent.
    fn keep(mut self) {
        self.ids.clear();
    }
}

impl Drop for Orphans {
    fn drop(&mut self) {
        if self.ids.is_empty() {
            return;
        }
        let connection = std::sync::Arc::clone(&self.connection);
        let peer = self.peer.clone();
        let ids = std::mem::take(&mut self.ids);

        // Spawned, because a destructor cannot wait. The connection is
        // behind an `Arc` that this task holds, so it stays open long
        // enough to finish even though the request is already gone.
        tokio::spawn(async move {
            tracing::warn!(parts = ids.len(), "removing parts of an interrupted upload");
            for batch in ids.chunks(FORWARD_BATCH) {
                if let Err(error) = connection.client.delete_messages(&peer, batch).await {
                    tracing::warn!(?error, "could not remove the parts of an interrupted upload");
                }
            }
        });
    }
}

/// A reader that digests everything passing through it.
///
/// The hash has to come from the same pass as the upload: the body is a
/// network stream that cannot be rewound, so there is no second chance to
/// read it.
struct Digesting<R> {
    inner: R,
    hasher: sha2::Sha256,
}

impl<R: tokio::io::AsyncRead + Unpin> tokio::io::AsyncRead for Digesting<R> {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let before = buf.filled().len();
        let this = self.as_mut().get_mut();
        let polled = std::pin::Pin::new(&mut this.inner).poll_read(cx, buf);
        if let std::task::Poll::Ready(Ok(())) = &polled {
            sha2::Digest::update(&mut this.hasher, &buf.filled()[before..]);
        }
        polled
    }
}

#[derive(Deserialize)]
pub struct MoveRequest {
    pub message_ids: Vec<i32>,
    /// Where they are now. Absent means Saved Messages.
    #[serde(default)]
    pub from: Option<Uuid>,
    /// Where they should end up. Absent means Saved Messages.
    #[serde(default)]
    pub to: Option<Uuid>,
}

/// Telegram's own ceiling for one `messages.forwardMessages` call.
const FORWARD_BATCH: usize = 100;

/// Moves files between folders.
///
/// Telegram has no move: a message belongs to the chat it was sent to. So
/// this forwards to the destination and deletes the original — and the
/// forward happens entirely on Telegram's servers, meaning a 2 GB file
/// moves in a moment and no bytes cross this server or the browser.
///
/// Two consequences worth knowing, both inherent rather than chosen: the
/// copy is a new message, so its id and its date are new; and it carries
/// Telegram's "forwarded from" marker when viewed in the Telegram app.
///
/// The original is deleted only for files that actually arrived. A partial
/// failure therefore leaves duplicates, never a hole.
async fn move_files(
    request: HttpRequest,
    user: CurrentUser,
    body: web::Json<MoveRequest>,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    // A move is both halves at once: something appears in one folder and
    // vanishes from another. Someone allowed only one of those should not
    // get the pair by combining them.
    permissions::require(&pool, user.id, permissions::Capability::Upload).await?;
    permissions::require(&pool, user.id, permissions::Capability::Delete).await?;

    let MoveRequest { message_ids, from, to } = body.into_inner();

    if message_ids.is_empty() {
        return Err(ApiError::BadRequest("Nothing was selected.".into()));
    }
    if from == to {
        return Err(ApiError::BadRequest(
            "Those files are already in that folder.".into(),
        ));
    }

    let connection = connection_for(&request, &user, &keys, &clients, &pool).await?;
    // Resolved before anything moves: a wrong destination should fail while
    // the files are still untouched.
    let source = super::folders::resolve(&connection.client, &pool, user.id, from).await?;
    let destination = super::folders::resolve(&connection.client, &pool, user.id, to).await?;

    let mut moved = 0usize;
    let mut failed = 0usize;

    // A file stored in several messages cannot just be forwarded: the
    // copies get new ids, so the index naming them would point back at the
    // originals. Those are moved one at a time, re-indexed on arrival, and
    // taken out of the ordinary batch below.
    //
    // Which ones they are is settled in one pass over the whole selection
    // rather than a lookup per file, since a hundred round trips before
    // anything moves is a hundred round trips.
    let selected = super::split::fetch_by_id(&connection.client, &source, &message_ids).await?;
    let mut plain = Vec::with_capacity(message_ids.len());

    for (id, found) in message_ids.iter().zip(selected.into_iter()) {
        let manifest = found.as_ref().filter(|m| m.media().is_none()).and_then(|m| {
            super::split::parse_manifest(m.text())
        });
        let Some(manifest) = manifest else {
            plain.push(*id);
            continue;
        };

        match move_split(&connection, &source, &destination, &manifest, *id).await {
            Ok(new_id) => {
                moved += 1;
                repoint(&pool, user.id, *id, new_id, from, to).await;
            }
            Err(error) => {
                tracing::warn!(?error, message_id = id, "could not move a split file");
                failed += 1;
            }
        }
    }
    let message_ids = plain;

    for batch in message_ids.chunks(FORWARD_BATCH) {
        let forwarded = connection
            .client
            .forward_messages(&destination, batch, &source)
            .await
            .map_err(|error| TelegramError::Other(format!("could not move the files: {error}")))?;

        // Paired by position: Telegram answers in the order it was asked,
        // with a gap where a message could not be forwarded.
        let mut landed = Vec::new();
        for (old_id, result) in batch.iter().zip(forwarded.into_iter()) {
            match result {
                Some(message) => landed.push((*old_id, message.id())),
                None => failed += 1,
            }
        }

        if landed.is_empty() {
            continue;
        }

        let originals: Vec<i32> = landed.iter().map(|(old, _)| *old).collect();
        connection
            .client
            .delete_messages(&source, &originals)
            .await
            .map_err(|error| {
                // The copies exist by now, so this leaves duplicates rather
                // than losing anything — say which half failed.
                TelegramError::Other(format!(
                    "the files were copied but the originals could not be removed: {error}"
                ))
            })?;

        moved += landed.len();

        for (old_id, new_id) in &landed {
            repoint(&pool, user.id, *old_id, *new_id, from, to).await;
        }
    }

    tracing::info!(user = %user.id, moved, failed, ?from, ?to, "moved files");

    Ok(HttpResponse::Ok().json(serde_json::json!({
        "ok": true,
        "moved": moved,
        "failed": failed,
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

    // A large file is a manifest plus its parts. Deleting only the
    // manifest would leave the parts behind, invisible and still paying
    // for themselves in storage, so they go first — the manifest last, so
    // that a failure midway leaves something that still names what is
    // missing rather than a set of orphans with nothing pointing at them.
    let mut doomed = vec![message_id];
    if let Some(manifest) = manifest_at(&connection, &peer, message_id).await {
        doomed = manifest.part_ids;
        doomed.push(message_id);
    }

    for batch in doomed.chunks(FORWARD_BATCH) {
        connection
            .client
            .delete_messages(&peer, batch)
            .await
            .map_err(|error| TelegramError::Other(format!("could not delete: {error}")))?;
    }

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

/// Points a file's quota row at where it has just been moved to.
///
/// Best effort: the move has already happened, and a file the tally has
/// lost track of is a smaller problem than an error for a move that
/// worked.
async fn repoint(
    pool: &Db,
    user: Uuid,
    old_id: i32,
    new_id: i32,
    from: Option<Uuid>,
    to: Option<Uuid>,
) {
    let Ok(db) = pool.get().await else { return };
    let updated = db
        .execute(
            "UPDATE uploads SET folder_id = $1, message_id = $2
              WHERE user_id = $3
                AND message_id = $4
                AND folder_id IS NOT DISTINCT FROM $5",
            &[&to, &new_id, &user, &old_id, &from],
        )
        .await;
    if let Err(error) = updated {
        tracing::warn!(?error, "moved a file but could not re-point its quota row");
    }
}

/// Moves one file that is stored as several messages.
///
/// The parts are forwarded, a fresh index naming the copies is sent to the
/// destination, and only then is the original removed. Ordered that way on
/// purpose: every intermediate state is one where the file is readable
/// somewhere. The worst outcome is a duplicate, never a file in pieces.
async fn move_split(
    connection: &Connection,
    source: &grammers_client::types::Peer,
    destination: &grammers_client::types::Peer,
    manifest: &super::split::Manifest,
    manifest_id: i32,
) -> ApiResult<i32> {
    let mut new_ids = Vec::with_capacity(manifest.part_ids.len());

    for batch in manifest.part_ids.chunks(FORWARD_BATCH) {
        let forwarded = connection
            .client
            .forward_messages(destination, batch, source)
            .await
            .map_err(|error| {
                TelegramError::Other(format!("could not copy the file's parts: {error}"))
            })?;

        for result in forwarded {
            let Some(message) = result else {
                // A part that did not arrive means the copy is incomplete.
                // Stop before writing an index for it: the parts that did
                // arrive are orphans, which cost storage, while the
                // original stays whole and readable.
                return Err(ApiError::Conflict(format!(
                    "\"{}\" could not be copied in full, so it has been left where it was.",
                    manifest.name
                )));
            };
            new_ids.push(message.id());
        }
    }

    let moved = super::split::Manifest {
        part_ids: new_ids,
        ..manifest.clone()
    };
    let written = connection
        .client
        .send_message(
            destination,
            InputMessage::from(super::split::manifest_text(&moved)?.as_str()),
        )
        .await
        .map_err(|error| {
            TelegramError::Other(format!("could not write the copied file's index: {error}"))
        })?;

    // The copy is complete and readable from here on, so the original can
    // go. Parts first, then the index, matching `remove`.
    let mut doomed = manifest.part_ids.clone();
    doomed.push(manifest_id);
    for batch in doomed.chunks(FORWARD_BATCH) {
        connection
            .client
            .delete_messages(source, batch)
            .await
            .map_err(|error| {
                TelegramError::Other(format!(
                    "\"{}\" was copied, but the original could not be removed: {error}",
                    manifest.name
                ))
            })?;
    }

    Ok(written.id())
}

/// Reads a message, answering with its manifest if it turns out to be one.
///
/// Best effort on purpose: every caller already has a sensible thing to do
/// when this says no, and a message that cannot be fetched is not a reason
/// to fail an operation that would otherwise work on it directly.
async fn manifest_at(
    connection: &Connection,
    peer: &grammers_client::types::Peer,
    message_id: i32,
) -> Option<super::split::Manifest> {
    let messages = connection
        .client
        .get_messages_by_id(peer, &[message_id])
        .await
        .ok()?;
    let message = messages.into_iter().flatten().next()?;
    if message.media().is_some() {
        return None;
    }
    super::split::parse_manifest(message.text())
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
