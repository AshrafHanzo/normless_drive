//! Serving a Telegram document over HTTP, whole or by byte range.
//!
//! Range support is not a nicety. It is what lets the browser read a zip's
//! table of contents — the last few kilobytes of the archive — and then pull
//! one file out of the middle of it, without downloading the whole thing.
//! Telegram is a chunked-read protocol underneath (`upload.getFile` takes an
//! offset), so serving part of a file costs no more than serving the start.
//!
//! The chunk size is fixed at Telegram's maximum. Its API requires the offset
//! to be a multiple of 4 KiB and a read never to cross a 1 MiB boundary, both
//! of which 512 KiB chunks at 512 KiB offsets satisfy by construction. An
//! arbitrary start lands inside a chunk, so the leading bytes of the first
//! one are dropped and the trailing bytes of the last are truncated.

use actix_web::http::{header, StatusCode};
use actix_web::{web, HttpRequest, HttpResponse};
use futures::Stream;
use grammers_client::types::Media;
use grammers_client::Client;

/// Telegram's per-request ceiling, and the default grammers uses.
const CHUNK: u64 = 512 * 1024;

/// An inclusive byte range, already clamped to the file.
#[derive(Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

impl Span {
    fn len(&self) -> u64 {
        self.end - self.start + 1
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Requested {
    Whole,
    Part(Span),
    /// The client asked for bytes that do not exist. Answering with the whole
    /// file would look like success to code that is paging through one.
    Unsatisfiable,
}

/// Reads a `Range` header.
///
/// Anything malformed, or a multi-range request, is treated as no range at
/// all: a server is always allowed to answer with the whole representation,
/// and that beats refusing a request we merely do not understand.
pub fn parse(raw: Option<&str>, size: u64) -> Requested {
    let Some(spec) = raw.and_then(|value| value.trim().strip_prefix("bytes=")) else {
        return Requested::Whole;
    };
    if spec.contains(',') || size == 0 {
        return Requested::Whole;
    }
    let Some((first, last)) = spec.split_once('-') else {
        return Requested::Whole;
    };

    // `bytes=-500` means the final 500 bytes, not "from 0 to 500". This is
    // the form a zip reader uses to find the central directory.
    if first.is_empty() {
        let Ok(suffix) = last.trim().parse::<u64>() else {
            return Requested::Whole;
        };
        if suffix == 0 {
            return Requested::Unsatisfiable;
        }
        return Requested::Part(Span {
            start: size.saturating_sub(suffix),
            end: size - 1,
        });
    }

    let Ok(start) = first.trim().parse::<u64>() else {
        return Requested::Whole;
    };
    if start >= size {
        return Requested::Unsatisfiable;
    }
    let end = if last.trim().is_empty() {
        size - 1
    } else {
        match last.trim().parse::<u64>() {
            Ok(value) => value.min(size - 1),
            Err(_) => return Requested::Whole,
        }
    };
    if end < start {
        return Requested::Unsatisfiable;
    }
    Requested::Part(Span { start, end })
}

/// Trims one downloaded chunk down to the part that was actually asked for.
///
/// Reads start at a chunk boundary, so the first chunk carries bytes before
/// the requested range and the last carries bytes after it. `None` means
/// this chunk lies entirely before the range and nothing should be sent.
fn splice(mut chunk: Vec<u8>, drop_prefix: &mut usize, remaining: &mut u64) -> Option<Vec<u8>> {
    if *drop_prefix > 0 {
        if *drop_prefix >= chunk.len() {
            *drop_prefix -= chunk.len();
            return None;
        }
        chunk.drain(..*drop_prefix);
        *drop_prefix = 0;
    }
    if chunk.len() as u64 > *remaining {
        chunk.truncate(*remaining as usize);
    }
    *remaining -= chunk.len() as u64;
    Some(chunk)
}

/// Streams `length` bytes starting at `start`, pulling from Telegram.
fn body(
    client: Client,
    media: Media,
    start: u64,
    length: u64,
    message_id: i32,
) -> impl Stream<Item = Result<web::Bytes, actix_web::Error>> {
    let skip = (start / CHUNK).min(i32::MAX as u64) as i32;
    let mut drop_prefix = (start % CHUNK) as usize;
    let mut remaining = length;

    async_stream::stream! {
        // Order matters: `skip_chunks` multiplies by the chunk size that is
        // set at the time it is called.
        let mut chunks = client
            .iter_download(&media)
            .chunk_size(CHUNK as i32)
            .skip_chunks(skip);

        while remaining > 0 {
            match chunks.next().await {
                Ok(Some(chunk)) => {
                    if let Some(wanted) = splice(chunk, &mut drop_prefix, &mut remaining) {
                        yield Ok::<_, actix_web::Error>(web::Bytes::from(wanted));
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    // The headers are long gone by now, so this cannot become
                    // a clean HTTP error — the transfer just ends short. Log
                    // it so a truncated download is explainable.
                    tracing::warn!(?error, message_id, "download stream failed mid-transfer");
                    break;
                }
            }
        }
    }
}

/// Builds the response for a document, honouring `Range` if it is present.
///
/// Nothing is buffered in either case: a 2 GB file leaves through the socket
/// as it arrives from Telegram.
pub fn serve(
    request: &HttpRequest,
    client: Client,
    media: Media,
    size: i64,
    mime: String,
    disposition: String,
    message_id: i32,
) -> HttpResponse {
    let total = size.max(0) as u64;
    let raw = request
        .headers()
        .get(header::RANGE)
        .and_then(|value| value.to_str().ok());

    match parse(raw, total) {
        Requested::Unsatisfiable => HttpResponse::build(StatusCode::RANGE_NOT_SATISFIABLE)
            .insert_header((header::CONTENT_RANGE, format!("bytes */{total}")))
            .insert_header((header::ACCEPT_RANGES, "bytes"))
            .finish(),

        Requested::Whole => HttpResponse::Ok()
            .content_type(mime)
            .insert_header((header::CONTENT_DISPOSITION, disposition))
            .insert_header((header::CONTENT_LENGTH, total.to_string()))
            // Advertised on the full response too — that is how a client
            // learns it may ask for a range next time.
            .insert_header((header::ACCEPT_RANGES, "bytes"))
            .streaming(Box::pin(body(client, media, 0, total, message_id))),

        Requested::Part(span) => {
            let length = span.len();
            HttpResponse::build(StatusCode::PARTIAL_CONTENT)
                .content_type(mime)
                .insert_header((header::CONTENT_DISPOSITION, disposition))
                .insert_header((
                    header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", span.start, span.end, total),
                ))
                .insert_header((header::CONTENT_LENGTH, length.to_string()))
                .insert_header((header::ACCEPT_RANGES, "bytes"))
                .streaming(Box::pin(body(client, media, span.start, length, message_id)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse, splice, Requested, Span, CHUNK};

    fn part(start: u64, end: u64) -> Requested {
        Requested::Part(Span { start, end })
    }

    /// Replays what the stream does: whole chunks arrive from a given
    /// boundary, and only the asked-for window should come out.
    fn served(file: &[u8], start: u64, length: u64) -> Vec<u8> {
        let chunk = 8usize; // stands in for CHUNK, small enough to read
        let mut drop_prefix = (start % chunk as u64) as usize;
        let mut remaining = length;
        let mut out = Vec::new();

        let mut at = (start / chunk as u64) as usize * chunk;
        while remaining > 0 && at < file.len() {
            let end = (at + chunk).min(file.len());
            if let Some(part) = splice(file[at..end].to_vec(), &mut drop_prefix, &mut remaining) {
                out.extend_from_slice(&part);
            }
            at = end;
        }
        out
    }

    #[test]
    fn a_range_is_cut_out_of_the_chunks_that_contain_it() {
        let file: Vec<u8> = (0u8..=99).collect();

        // Whole file.
        assert_eq!(served(&file, 0, 100), file);
        // Starting mid-chunk, ending mid-chunk.
        assert_eq!(served(&file, 3, 10), file[3..13]);
        // A window that skips several chunks before it begins.
        assert_eq!(served(&file, 41, 7), file[41..48]);
        // Exactly on boundaries.
        assert_eq!(served(&file, 8, 8), file[8..16]);
        // The tail, which is how a zip's index is read.
        assert_eq!(served(&file, 95, 5), file[95..]);
        // One byte.
        assert_eq!(served(&file, 50, 1), file[50..51]);
    }

    #[test]
    fn the_chunk_size_is_one_telegram_will_accept() {
        // Its API wants a multiple of 4 KiB that never straddles a 1 MiB
        // boundary; both follow from this and it must stay true.
        assert_eq!(CHUNK % 4096, 0);
        assert_eq!(1024 * 1024 % CHUNK, 0);
    }

    #[test]
    fn an_absent_or_odd_header_asks_for_everything() {
        assert_eq!(parse(None, 1000), Requested::Whole);
        assert_eq!(parse(Some("items=0-10"), 1000), Requested::Whole);
        assert_eq!(parse(Some("bytes=abc"), 1000), Requested::Whole);
        // Multi-range is legal but needs a multipart body; answering whole
        // is a valid response to it.
        assert_eq!(parse(Some("bytes=0-10,20-30"), 1000), Requested::Whole);
    }

    #[test]
    fn a_closed_range_is_clamped_to_the_file() {
        assert_eq!(parse(Some("bytes=0-99"), 1000), part(0, 99));
        assert_eq!(parse(Some("bytes=900-99999"), 1000), part(900, 999));
    }

    #[test]
    fn an_open_range_runs_to_the_end() {
        assert_eq!(parse(Some("bytes=500-"), 1000), part(500, 999));
    }

    /// The form a zip reader uses: give me the last N bytes, where the
    /// central directory lives.
    #[test]
    fn a_suffix_range_counts_back_from_the_end() {
        assert_eq!(parse(Some("bytes=-64"), 1000), part(936, 999));
        // Asking for more tail than exists is the whole file, not an error.
        assert_eq!(parse(Some("bytes=-5000"), 1000), part(0, 999));
    }

    #[test]
    fn a_range_past_the_end_is_refused() {
        assert_eq!(parse(Some("bytes=1000-"), 1000), Requested::Unsatisfiable);
        assert_eq!(parse(Some("bytes=-0"), 1000), Requested::Unsatisfiable);
    }
}
