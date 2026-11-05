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
//!
//! A file may be more than one document. Anything over Telegram's 2 GB
//! ceiling is stored as several of them (see `split`), and this serves the
//! sequence as one continuous file: a range starting in the middle of part
//! three begins its reads there. Each part is a separate Telegram file with
//! its own offsets starting at zero, so the chunk arithmetic is done per
//! part rather than once over the whole.

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

/// One Telegram document, and how many of the logical file's bytes it holds.
///
/// An ordinary file is a single source. A split file is several, in order.
pub struct Source {
    pub media: Media,
    pub len: u64,
}

impl Source {
    pub fn single(media: Media, size: i64) -> Vec<Self> {
        vec![Source {
            media,
            len: size.max(0) as u64,
        }]
    }
}

/// Which source a byte of the logical file lives in, and how far into it.
///
/// Returns one past the last source for an offset beyond the end, rather
/// than panicking: a caller that is off by one gets an empty read instead
/// of a crash.
///
/// Takes lengths rather than sources so it can be tested without a live
/// Telegram document to build a `Media` from.
fn locate(lengths: &[u64], at: u64) -> (usize, u64) {
    let mut consumed = 0u64;
    for (index, len) in lengths.iter().enumerate() {
        if at < consumed + len {
            return (index, at - consumed);
        }
        consumed += len;
    }
    (lengths.len(), 0)
}

/// Streams `length` bytes starting at `start`, pulling from Telegram.
fn body(
    client: Client,
    sources: Vec<Source>,
    start: u64,
    length: u64,
    message_id: i32,
) -> impl Stream<Item = Result<web::Bytes, actix_web::Error>> {
    let lengths: Vec<u64> = sources.iter().map(|source| source.len).collect();

    async_stream::stream! {
        let (mut index, mut offset) = locate(&lengths, start);
        let mut remaining = length;

        while remaining > 0 && index < sources.len() {
            let source = &sources[index];
            let skip = (offset / CHUNK).min(i32::MAX as u64) as i32;
            let mut drop_prefix = (offset % CHUNK) as usize;
            // How much of what is still wanted lies inside this part.
            let mut wanted_here = (source.len - offset).min(remaining);

            // Order matters: `skip_chunks` multiplies by the chunk size that
            // is set at the time it is called.
            let mut chunks = client
                .iter_download(&source.media)
                .chunk_size(CHUNK as i32)
                .skip_chunks(skip);

            while wanted_here > 0 {
                match chunks.next().await {
                    Ok(Some(chunk)) => {
                        if let Some(piece) = splice(chunk, &mut drop_prefix, &mut wanted_here) {
                            remaining -= piece.len() as u64;
                            yield Ok::<_, actix_web::Error>(web::Bytes::from(piece));
                        }
                    }
                    // The part ran out before it had given what it was
                    // recorded as holding. Moving to the next one would
                    // splice its bytes into the gap and hand back a file
                    // that is wrong rather than short, so stop here.
                    Ok(None) => {
                        tracing::warn!(
                            message_id,
                            part = index,
                            short_by = wanted_here,
                            "a file part ended early"
                        );
                        return;
                    }
                    Err(error) => {
                        // The headers are long gone by now, so this cannot
                        // become a clean HTTP error — the transfer just ends
                        // short. Log it so a truncated download is
                        // explainable.
                        tracing::warn!(?error, message_id, "download stream failed mid-transfer");
                        return;
                    }
                }
            }

            index += 1;
            offset = 0;
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
    sources: Vec<Source>,
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
            .streaming(Box::pin(body(client, sources, 0, total, message_id))),

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
                .streaming(Box::pin(body(client, sources, span.start, length, message_id)))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{locate, parse, splice, Requested, Span, CHUNK};

    #[test]
    fn an_offset_resolves_to_the_part_that_holds_it() {
        let parts = [100u64, 100, 50];

        assert_eq!(locate(&parts, 0), (0, 0));
        assert_eq!(locate(&parts, 99), (0, 99));
        // The first byte of the second part. An off-by-one here would
        // re-send a whole part or skip one.
        assert_eq!(locate(&parts, 100), (1, 0));
        assert_eq!(locate(&parts, 150), (1, 50));
        assert_eq!(locate(&parts, 200), (2, 0));
        assert_eq!(locate(&parts, 249), (2, 49));
        // Past the end: one past the last part, never a panic.
        assert_eq!(locate(&parts, 250), (3, 0));
        assert_eq!(locate(&parts, 9999), (3, 0));
    }

    #[test]
    fn a_single_part_behaves_as_it_always_did() {
        assert_eq!(locate(&[1000], 0), (0, 0));
        assert_eq!(locate(&[1000], 999), (0, 999));
    }

    /// Replays what `body` does across several parts: each part restarts
    /// its own chunking at zero, and the pieces must join into exactly the
    /// requested window of the logical file.
    #[test]
    fn a_range_spanning_parts_is_rebuilt_exactly() {
        let chunk = 8usize;
        // A file of 250 bytes stored as three parts.
        let file: Vec<u8> = (0..250u32).map(|n| (n % 251) as u8).collect();
        let bounds = [(0usize, 100usize), (100, 100), (200, 50)];

        let served = |start: u64, length: u64| -> Vec<u8> {
            let sizes: Vec<u64> = bounds.iter().map(|(_, len)| *len as u64).collect();
            // Inlined `locate`, over plain lengths.
            let (mut index, mut offset) = {
                let mut consumed = 0u64;
                let mut found = (sizes.len(), 0);
                for (i, len) in sizes.iter().enumerate() {
                    if start < consumed + len {
                        found = (i, start - consumed);
                        break;
                    }
                    consumed += len;
                }
                found
            };

            let mut remaining = length;
            let mut out = Vec::new();

            while remaining > 0 && index < bounds.len() {
                let (base, len) = bounds[index];
                let mut drop_prefix = (offset % chunk as u64) as usize;
                let mut wanted_here = (len as u64 - offset).min(remaining);

                let mut at = (offset / chunk as u64) as usize * chunk;
                while wanted_here > 0 && at < len {
                    let end = (at + chunk).min(len);
                    let chunk_bytes = file[base + at..base + end].to_vec();
                    if let Some(piece) = splice(chunk_bytes, &mut drop_prefix, &mut wanted_here) {
                        remaining -= piece.len() as u64;
                        out.extend_from_slice(&piece);
                    }
                    at = end;
                }
                index += 1;
                offset = 0;
            }
            out
        };

        // The whole file, reassembled.
        assert_eq!(served(0, 250), file);
        // Entirely inside the first part.
        assert_eq!(served(5, 20), file[5..25]);
        // Across the first boundary, starting mid-chunk.
        assert_eq!(served(95, 20), file[95..115]);
        // Across both boundaries.
        assert_eq!(served(90, 130), file[90..220]);
        // Starting exactly on a boundary.
        assert_eq!(served(100, 10), file[100..110]);
        // Entirely inside the last part.
        assert_eq!(served(210, 10), file[210..220]);
        // The tail, which is how a zip's index is read — and the case that
        // matters for looking inside a split archive.
        assert_eq!(served(240, 10), file[240..]);
        // One byte, on each side of a boundary.
        assert_eq!(served(99, 1), file[99..100]);
        assert_eq!(served(100, 1), file[100..101]);
    }

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
