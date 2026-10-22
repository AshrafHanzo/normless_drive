//! Remote upload: give the server a URL, it fetches the file and puts it
//! in your Telegram without the bytes ever passing through your browser.
//!
//! The desktop app has the same feature. Here it matters more: the server
//! usually has far better bandwidth than whoever is using it, so pulling a
//! large file this way is often the difference between minutes and hours.

use actix_web::{web, HttpRequest, HttpResponse};
use futures::StreamExt;
use grammers_client::InputMessage;
use serde::Deserialize;
use uuid::Uuid;

use super::{connect::SharedClients, TelegramError};
use crate::{
    auth::CurrentUser,
    db::Db,
    error::{ApiError, ApiResult},
    keycache::KeyCache,
    permissions,
};

pub fn configure(cfg: &mut web::ServiceConfig) {
    cfg.service(web::resource("/api/files/remote").route(web::post().to(remote_upload)));
}

#[derive(Deserialize)]
pub struct RemoteRequest {
    pub url: String,
    #[serde(default)]
    pub folder: Option<Uuid>,
    /// Overrides the name derived from the URL or Content-Disposition.
    #[serde(default)]
    pub name: Option<String>,
}

async fn remote_upload(
    request: HttpRequest,
    user: CurrentUser,
    body: web::Json<RemoteRequest>,
    keys: web::Data<KeyCache>,
    clients: web::Data<SharedClients>,
    pool: web::Data<Db>,
) -> ApiResult<HttpResponse> {
    permissions::require(&pool, user.id, permissions::Capability::Upload).await?;

    let url = validate_url(&body.url).map_err(ApiError::BadRequest)?;

    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|error| ApiError::Internal(anyhow::anyhow!(error)))?;

    let response = http
        .get(url.clone())
        .send()
        .await
        .map_err(|error| ApiError::BadRequest(format!("Could not fetch that URL: {error}")))?;

    if !response.status().is_success() {
        return Err(ApiError::BadRequest(format!(
            "That URL answered {}.",
            response.status().as_u16()
        )));
    }

    // An HTML response is almost always a download gateway or a login
    // page, not the file. Uploading it would store a few KB of markup
    // under a plausible filename, which is worse than failing.
    if let Some(content_type) = response.headers().get(reqwest::header::CONTENT_TYPE) {
        let value = content_type.to_str().unwrap_or_default().to_lowercase();
        if value.contains("text/html") {
            return Err(ApiError::BadRequest(
                "That URL returned a web page, not a file. Use a direct download link.".into(),
            ));
        }
    }

    // Telegram needs the length before the transfer starts, and a chunked
    // response does not carry one. Nothing can be done about that here
    // short of buffering the whole file, which is what this feature exists
    // to avoid.
    let size = response.content_length().ok_or_else(|| {
        ApiError::BadRequest(
            "That server did not say how large the file is, which Telegram requires.".into(),
        )
    })?;

    if size == 0 {
        return Err(ApiError::BadRequest("That file is empty.".into()));
    }
    const TELEGRAM_MAX: u64 = 2_000_000_000;
    if size > TELEGRAM_MAX {
        return Err(ApiError::BadRequest(format!(
            "Telegram accepts at most 2 GB in one file. That one is {:.2} GB.",
            size as f64 / 1_000_000_000.0
        )));
    }

    permissions::check_quota(&pool, user.id, size as i64).await?;

    let name = body
        .name
        .as_deref()
        .map(str::to_string)
        .or_else(|| filename_from(&response))
        .unwrap_or_else(|| filename_from_url(&url));

    let connection = super::files::connection_for(&request, &user, &keys, &clients, &pool).await?;
    let peer = super::folders::resolve(&connection.client, &pool, user.id, body.folder).await?;

    let stream = response
        .bytes_stream()
        .map(|chunk| chunk.map_err(std::io::Error::other));
    let mut reader = tokio_util::io::StreamReader::new(stream);

    let uploaded = connection
        .client
        .upload_stream(&mut reader, size as usize, name.clone())
        .await
        .map_err(|error| TelegramError::Other(format!("upload to Telegram failed: {error}")))?;

    let message = connection
        .client
        .send_message(&peer, InputMessage::from("").document(uploaded))
        .await
        .map_err(|error| TelegramError::Other(format!("could not attach the file: {error}")))?;

    if let Ok(db) = pool.get().await {
        let _ = db
            .execute(
                "INSERT INTO uploads (user_id, folder_id, message_id, name, size_bytes)
                 VALUES ($1, $2, $3, $4, $5)",
                &[&user.id, &body.folder, &message.id(), &name, &(size as i64)],
            )
            .await;
    }

    tracing::info!(user = %user.id, name = %name, size, "remote upload");
    Ok(HttpResponse::Created().json(serde_json::json!({
        "ok": true, "name": name, "size": size
    })))
}

/// Rejects anything that is not a public http(s) URL.
///
/// Without this the endpoint is a server-side request forgery hole: the
/// server would happily fetch `http://169.254.169.254/` (cloud metadata),
/// `http://127.0.0.1:5432` or anything else on the private network, and
/// hand the contents back as a file.
fn validate_url(raw: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(raw.trim())
        .map_err(|_| "That does not look like a URL.".to_string())?;

    match url.scheme() {
        "http" | "https" => {}
        other => return Err(format!("{other}:// links are not supported.")),
    }

    let host = url
        .host_str()
        .ok_or_else(|| "That URL has no host.".to_string())?
        .to_ascii_lowercase();

    let blocked = host == "localhost"
        || host == "::1"
        || host.ends_with(".localhost")
        || host.ends_with(".internal")
        || host.starts_with("127.")
        || host.starts_with("0.")
        || host.starts_with("10.")
        || host.starts_with("192.168.")
        // Cloud metadata. Reaching this would hand over instance
        // credentials on most providers.
        || host.starts_with("169.254.")
        || (host.starts_with("172.")
            && host
                .split('.')
                .nth(1)
                .and_then(|octet| octet.parse::<u8>().ok())
                .is_some_and(|octet| (16..=31).contains(&octet)));

    if blocked {
        return Err("That address is on a private network and cannot be fetched.".into());
    }

    Ok(url)
}

fn filename_from(response: &reqwest::Response) -> Option<String> {
    let header = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)?
        .to_str()
        .ok()?;

    // RFC 6266: filename*=UTF-8''... takes precedence over filename="..."
    if let Some(encoded) = header.split("filename*=").nth(1) {
        let value = encoded.trim().trim_matches('"');
        let decoded = value.rsplit("''").next().unwrap_or(value);
        let name = urlencoding_decode(decoded);
        if !name.is_empty() {
            return Some(super::files::sanitize_name(&name));
        }
    }

    let value = header.split("filename=").nth(1)?.trim().trim_matches('"');
    let name = super::files::sanitize_name(value);
    (!name.is_empty()).then_some(name)
}

fn filename_from_url(url: &reqwest::Url) -> String {
    let candidate = url
        .path_segments()
        .and_then(|segments| segments.last())
        .map(urlencoding_decode)
        .unwrap_or_default();
    let cleaned = super::files::sanitize_name(&candidate);
    if cleaned.is_empty() {
        "download".to_string()
    } else {
        cleaned
    }
}

/// Minimal percent-decoding. Only used on filenames, where the alternative
/// is showing the user `My%20Report.pdf`.
fn urlencoding_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Ok(byte) = u8::from_str_radix(&value[index + 1..index + 3], 16) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_and_metadata_addresses_are_refused() {
        // Each of these would turn this endpoint into a way to read the
        // server's own network from outside.
        for hostile in [
            "http://localhost/secret",
            "http://127.0.0.1:5432/",
            "http://169.254.169.254/latest/meta-data/",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            "http://172.31.255.255/",
            "http://db.internal/dump",
        ] {
            assert!(validate_url(hostile).is_err(), "should refuse {hostile}");
        }
    }

    #[test]
    fn public_addresses_that_merely_look_private_are_allowed() {
        for fine in ["https://172.15.0.1/f", "https://172.32.0.1/f", "https://example.com/f"] {
            assert!(validate_url(fine).is_ok(), "should allow {fine}");
        }
    }

    #[test]
    fn only_http_and_https_are_accepted() {
        for bad in ["file:///etc/passwd", "ftp://x.com/f", "gopher://x.com/"] {
            assert!(validate_url(bad).is_err(), "should refuse {bad}");
        }
    }

    #[test]
    fn a_filename_is_taken_from_the_url_path() {
        let url = reqwest::Url::parse("https://example.com/files/My%20Report.pdf").unwrap();
        assert_eq!(filename_from_url(&url), "My Report.pdf");
    }

    #[test]
    fn a_url_with_no_usable_name_falls_back() {
        let url = reqwest::Url::parse("https://example.com/").unwrap();
        assert_eq!(filename_from_url(&url), "download");
    }
}
