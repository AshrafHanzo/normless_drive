//! Files larger than one Telegram message.
//!
//! Telegram takes at most 2 GB in a single document, and a drive that
//! refuses anything bigger is not a drive. So a large file is uploaded as
//! several ordinary document messages — "parts" — followed by one plain
//! text message, the "manifest", carrying the original name and size and
//! the ordered list of part message ids.
//!
//! The manifest is what the file list shows: one row, the real name, the
//! real size. The parts are hidden, recognised by a marker in their
//! caption. A download reads the manifest, resolves its parts and streams
//! them back to back as one file, including for a byte range that lands in
//! the middle of part three.
//!
//! The format is not invented here. It is the desktop app's
//! (`split_file.rs` in Telegram-drive), reproduced byte for byte — same
//! markers, same JSON, same `part{:04}` document names, same en-dashed
//! caption — so a file split by one is a file the other can read. Changing
//! any of it in one place without the other would quietly break that.
//!
//! The manifest is sent only after every part has landed. A failure part
//! way through therefore leaves orphaned parts, which are invisible and
//! cost storage, rather than a manifest pointing at messages that do not
//! exist.

use grammers_client::types::{Media, Peer};
use grammers_client::Client;
use serde::{Deserialize, Serialize};

use super::TelegramError;
use crate::error::{ApiError, ApiResult};

/// Telegram's approximate 2 GB ceiling for one document.
pub const TELEGRAM_MAX_FILE_SIZE: u64 = 2_000_000_000;

pub const MANIFEST_MARKER: &str = "[TD-SPLIT]";
pub const PART_MARKER: &str = "[TD-SPLIT-PART]";

/// Telegram's own cap on ids per `get_messages_by_id`.
const MAX_IDS_PER_BATCH: usize = 100;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub schema_version: u16,
    pub name: String,
    pub size: u64,
    pub part_count: u32,
    pub part_ids: Vec<i32>,
    /// SHA-256 of the whole reassembled file, hex. Absent on manifests
    /// written by paths that cannot hash in one pass; those still parse and
    /// are simply not verifiable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// Reads a message's text as a manifest, if that is what it is.
///
/// Returns `None` for anything else without complaint: this runs against
/// every message in the chat, including notes the user wrote themselves and
/// messages from other apps sharing the folder.
pub fn parse_manifest(text: &str) -> Option<Manifest> {
    let json = text.strip_prefix(MANIFEST_MARKER)?;
    serde_json::from_str(json).ok()
}

/// True for the hidden document messages a split file is made of.
pub fn is_part(text: &str) -> bool {
    text.starts_with(PART_MARKER)
}

pub fn should_split(size: u64) -> bool {
    size > TELEGRAM_MAX_FILE_SIZE
}

/// Consecutive `(start, len)` ranges, each at most one message's worth.
pub fn part_ranges(total: u64) -> Vec<(u64, u64)> {
    if total == 0 {
        return vec![(0, 0)];
    }
    let mut ranges = Vec::new();
    let mut offset = 0u64;
    while offset < total {
        let len = TELEGRAM_MAX_FILE_SIZE.min(total - offset);
        ranges.push((offset, len));
        offset += len;
    }
    ranges
}

/// The document filename each part is uploaded under.
pub fn part_name(index: usize) -> String {
    format!("part{:04}", index + 1)
}

/// Cosmetic, for anyone browsing the chat in Telegram itself — but the
/// marker prefix is load-bearing, since it is what hides the part here.
pub fn part_caption(index: usize, total: usize, name: &str) -> String {
    format!("{} {}/{} \u{2013} {}", PART_MARKER, index + 1, total, name)
}

pub fn manifest_text(manifest: &Manifest) -> ApiResult<String> {
    let json = serde_json::to_string(manifest)
        .map_err(|error| TelegramError::Other(format!("could not write the manifest: {error}")))?;
    Ok(format!("{MANIFEST_MARKER}{json}"))
}

/// Fetches messages by id, in batches, keeping the order asked for.
///
/// An id that does not resolve becomes `None` at its position, so a caller
/// can say which part is missing rather than only that one is.
pub async fn fetch_by_id(
    client: &Client,
    peer: &Peer,
    ids: &[i32],
) -> ApiResult<Vec<Option<grammers_client::types::Message>>> {
    let mut out = Vec::with_capacity(ids.len());
    for batch in ids.chunks(MAX_IDS_PER_BATCH) {
        let messages = client.get_messages_by_id(peer, batch).await.map_err(|error| {
            TelegramError::Other(format!("could not fetch the file's parts: {error}"))
        })?;
        out.extend(messages);
    }
    Ok(out)
}

/// Resolves a manifest into the documents its parts carry, in order.
///
/// Every part must be present. A file served with a hole in the middle
/// would be silently corrupt, which is far worse than a refused download.
pub async fn resolve_parts(
    client: &Client,
    peer: &Peer,
    manifest: &Manifest,
) -> ApiResult<Vec<super::ranged::Source>> {
    let total = manifest.part_ids.len();
    let messages = fetch_by_id(client, peer, &manifest.part_ids).await?;

    let mut sources = Vec::with_capacity(total);
    for (index, found) in messages.into_iter().enumerate() {
        let document = found
            .as_ref()
            .and_then(|message| match message.media() {
                Some(Media::Document(document)) => Some(document),
                _ => None,
            })
            .ok_or_else(|| {
                ApiError::NotFound(format!(
                    "\"{}\" is missing part {} of {} — it cannot be put back together.",
                    manifest.name,
                    index + 1,
                    total
                ))
            })?;

        let len = document.size().max(0) as u64;
        sources.push(super::ranged::Source {
            media: Media::Document(document),
            len,
        });
    }

    Ok(sources)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAX: u64 = TELEGRAM_MAX_FILE_SIZE;

    #[test]
    fn ranges_cover_the_file_exactly_once() {
        let ranges = part_ranges(MAX * 2 + 500);
        assert_eq!(
            ranges,
            vec![(0, MAX), (MAX, MAX), (MAX * 2, 500)],
        );
        let covered: u64 = ranges.iter().map(|(_, len)| len).sum();
        assert_eq!(covered, MAX * 2 + 500);
    }

    #[test]
    fn an_exact_multiple_has_no_empty_tail() {
        assert_eq!(part_ranges(MAX * 2), vec![(0, MAX), (MAX, MAX)]);
    }

    #[test]
    fn one_byte_over_the_limit_is_two_parts() {
        assert_eq!(part_ranges(MAX + 1), vec![(0, MAX), (MAX, 1)]);
    }

    #[test]
    fn a_small_file_is_one_range() {
        assert_eq!(part_ranges(100), vec![(0, 100)]);
    }

    #[test]
    fn the_threshold_is_inclusive() {
        assert!(!should_split(MAX));
        assert!(should_split(MAX + 1));
    }

    #[test]
    fn a_manifest_round_trips() {
        let manifest = Manifest {
            schema_version: 1,
            name: "movie.mkv".into(),
            size: 5_000_000_000,
            part_count: 3,
            part_ids: vec![101, 102, 103],
            sha256: Some("ab12".into()),
        };
        let parsed = parse_manifest(&manifest_text(&manifest).unwrap()).unwrap();
        assert_eq!(parsed.name, "movie.mkv");
        assert_eq!(parsed.size, 5_000_000_000);
        assert_eq!(parsed.part_ids, vec![101, 102, 103]);
        assert_eq!(parsed.sha256.as_deref(), Some("ab12"));
    }

    /// Manifests the desktop app wrote before it recorded a digest have no
    /// `sha256` key at all, and must still parse.
    #[test]
    fn a_manifest_without_a_digest_still_parses() {
        let text = format!(
            "{MANIFEST_MARKER}{{\"schema_version\":1,\"name\":\"a.bin\",\"size\":9,\
             \"part_count\":1,\"part_ids\":[7]}}"
        );
        let parsed = parse_manifest(&text).expect("should parse");
        assert_eq!(parsed.sha256, None);
        assert_eq!(parsed.part_ids, vec![7]);
    }

    #[test]
    fn foreign_text_is_not_a_manifest() {
        assert!(parse_manifest("hello world").is_none());
        assert!(parse_manifest(MANIFEST_MARKER).is_none());
        assert!(parse_manifest(&format!("{MANIFEST_MARKER}not json")).is_none());
        // A part is not a manifest, even though both carry a marker.
        assert!(parse_manifest(&part_caption(0, 2, "x.zip")).is_none());
    }

    #[test]
    fn only_parts_are_recognised_as_parts() {
        assert!(is_part(&part_caption(0, 3, "x.zip")));
        assert!(!is_part(MANIFEST_MARKER));
        assert!(!is_part("holiday photos"));
    }

    /// The wire format is shared with the desktop app. These are the exact
    /// strings it writes; changing one here without changing it there makes
    /// files from the two apps unreadable to each other.
    #[test]
    fn the_wire_format_matches_the_desktop_app() {
        assert_eq!(MANIFEST_MARKER, "[TD-SPLIT]");
        assert_eq!(PART_MARKER, "[TD-SPLIT-PART]");
        assert_eq!(TELEGRAM_MAX_FILE_SIZE, 2_000_000_000);
        assert_eq!(part_name(0), "part0001");
        assert_eq!(part_name(11), "part0012");
        assert_eq!(
            part_caption(0, 3, "movie.mkv"),
            "[TD-SPLIT-PART] 1/3 \u{2013} movie.mkv"
        );
        assert_eq!(
            manifest_text(&Manifest {
                schema_version: 1,
                name: "a".into(),
                size: 1,
                part_count: 1,
                part_ids: vec![2],
                sha256: None,
            })
            .unwrap(),
            "[TD-SPLIT]{\"schema_version\":1,\"name\":\"a\",\"size\":1,\
             \"part_count\":1,\"part_ids\":[2]}"
        );
    }
}
