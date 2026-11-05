// Writing a zip one file at a time, without ever holding it whole.
//
// The old builder read every file into an array, handed the lot to fflate
// and got back one big Uint8Array — so a 4 GB folder needed 8 GB of tab
// memory and the archive was refused long before it was tried. This writes
// the archive straight through to a sink: headers, then the file's bytes as
// they are read, then the next file. Peak memory is one chunk.
//
// Every entry carries its sizes and CRC in a *data descriptor* after the
// data rather than in the local header, which is what general-purpose flag
// bit 3 means. That is the only way to stream: the compressed size is not
// known until the data has been through the compressor, and the CRC is not
// known until the file has been read.
//
// Archives past 4 GB, or past 65,535 entries, use zip64 — a second set of
// 64-bit fields carried in an extra field beside each 32-bit one that has
// been saturated to all-ones. It is written only where it is needed, which
// is what every reader expects: a small archive with zip64 records in it
// is legal but needlessly modern, and some old tools baulk.

import { Deflate } from "fflate";

export interface Sink {
    write(bytes: Uint8Array): Promise<void>;
}

export interface ZipEntry {
    /// Path inside the archive, relative to its root.
    path: string;
    file: File;
}

const STORED = 0;
const DEFLATED = 8;

/// Bit 3: sizes and CRC follow the data. Bit 11: the name is UTF-8.
const FLAGS = 0x0808;

const LOCAL_SIG = 0x04034b50;
const DESCRIPTOR_SIG = 0x08074b50;
const CENTRAL_SIG = 0x02014b50;
const END_SIG = 0x06054b50;
const END64_SIG = 0x06064b50;
const LOCATOR64_SIG = 0x07064b50;

/// The all-ones value a 32-bit zip field is set to when the real one lives
/// in a zip64 record. A value that large is therefore never stored
/// directly, which is why the comparisons below are `>=`, not `>`.
const FIELD_MAX = 0xffffffff;
const COUNT_MAX = 0xffff;

/// What a reader must support. 2.0 is the floor for deflate; 4.5 is the
/// floor for zip64, and claiming it on an archive that does not need it
/// would turn away readers that could have managed.
const NEEDS_BASE = 20;
const NEEDS_ZIP64 = 45;

/// The zip64 extra field's own header is four bytes: id, then length.
const EXTRA_HEADER = 4;

/// Extensions whose contents are already compressed.
///
/// Running deflate over them costs minutes on a multi-gigabyte folder and
/// gives back a fraction of a percent, so they are stored verbatim. The CRC
/// still has to be computed, but that is a single pass and cheap.
const INCOMPRESSIBLE = new Set([
    // Video
    "mp4", "m4v", "mov", "avi", "mkv", "webm", "mpg", "mpeg", "wmv", "flv",
    "3gp", "mts", "m2ts", "mxf", "braw", "r3d", "ts",
    // Audio
    "mp3", "aac", "m4a", "ogg", "oga", "opus", "flac", "wma",
    // Images, including camera raw
    "jpg", "jpeg", "png", "gif", "webp", "heic", "heif", "avif", "jxl",
    "arw", "cr2", "cr3", "nef", "dng", "orf", "raf", "rw2",
    // Already-compressed containers
    "zip", "gz", "tgz", "bz2", "xz", "zst", "br", "7z", "rar", "lz4",
    "docx", "xlsx", "pptx", "odt", "ods", "odp", "apk", "jar", "epub",
    "pdf",
]);

function methodFor(path: string): number {
    const dot = path.lastIndexOf(".");
    if (dot === -1) return DEFLATED;
    return INCOMPRESSIBLE.has(path.slice(dot + 1).toLowerCase())
        ? STORED
        : DEFLATED;
}

const CRC_TABLE = (() => {
    const table = new Int32Array(256);
    for (let n = 0; n < 256; n += 1) {
        let c = n;
        for (let k = 0; k < 8; k += 1) {
            c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
        }
        table[n] = c;
    }
    return table;
})();

/// Running CRC-32. Start from `-1` and finish with `(crc ^ -1) >>> 0`.
function crcUpdate(crc: number, bytes: Uint8Array): number {
    let c = crc;
    for (let i = 0; i < bytes.length; i += 1) {
        c = CRC_TABLE[(c ^ bytes[i]) & 0xff] ^ (c >>> 8);
    }
    return c;
}

/// MS-DOS packed date and time: years counted from 1980, seconds in steps
/// of two. Out-of-range dates are clamped rather than wrapped, which is
/// what happens if the arithmetic is left to overflow the bit fields.
function dosStamp(at: Date): { time: number; date: number } {
    const year = at.getFullYear();
    if (!Number.isFinite(year) || year < 1980) return { time: 0, date: 0x21 };
    if (year > 2107) return { time: 0xbf7d, date: 0xff9f };
    return {
        time:
            (at.getHours() << 11) | (at.getMinutes() << 5) | (at.getSeconds() >> 1),
        date: ((year - 1980) << 9) | ((at.getMonth() + 1) << 5) | at.getDate(),
    };
}

/// The shape every browser API uses to say "you stopped this", so callers
/// can tell a cancel apart from a failure with one check.
function cancelled(): DOMException {
    return new DOMException("The upload was cancelled.", "AbortError");
}

/// Zip paths use forward slashes and never start at a root.
function normalize(path: string): string {
    return path.replace(/\\/g, "/").replace(/^\/+/, "");
}

function wide(view: DataView, at: number, value: number) {
    view.setBigUint64(at, BigInt(value), true);
}

/// A local header, plus the zip64 extra field if this entry needs one.
///
/// The extra field has to be here, not only in the central directory: a
/// reader scanning forwards learns the descriptor's width from it, and
/// writing it is the only honest way to say "the sizes after the data are
/// eight bytes each".
function localHeader(
    name: Uint8Array,
    method: number,
    stamp: { time: number; date: number },
    zip64: boolean,
): Uint8Array {
    const extra = zip64 ? EXTRA_HEADER + 16 : 0;
    const out = new Uint8Array(30 + name.length + extra);
    const view = new DataView(out.buffer);
    view.setUint32(0, LOCAL_SIG, true);
    view.setUint16(4, zip64 ? NEEDS_ZIP64 : NEEDS_BASE, true);
    view.setUint16(6, FLAGS, true);
    view.setUint16(8, method, true);
    view.setUint16(10, stamp.time, true);
    view.setUint16(12, stamp.date, true);
    // CRC and both sizes stay zero here: they are not known yet, and the
    // descriptor written after the data carries the real values.
    if (zip64) {
        view.setUint32(18, FIELD_MAX, true);
        view.setUint32(22, FIELD_MAX, true);
    }
    view.setUint16(26, name.length, true);
    view.setUint16(28, extra, true);
    out.set(name, 30);

    if (zip64) {
        const at = 30 + name.length;
        view.setUint16(at, 0x0001, true);
        view.setUint16(at + 2, 16, true);
        // Placeholders, for the same reason the 32-bit fields are zero.
        wide(view, at + 4, 0);
        wide(view, at + 12, 0);
    }
    return out;
}

function descriptor(
    crc: number,
    compressed: number,
    uncompressed: number,
    zip64: boolean,
): Uint8Array {
    const out = new Uint8Array(zip64 ? 24 : 16);
    const view = new DataView(out.buffer);
    view.setUint32(0, DESCRIPTOR_SIG, true);
    view.setUint32(4, crc, true);
    if (zip64) {
        wide(view, 8, compressed);
        wide(view, 16, uncompressed);
    } else {
        view.setUint32(8, compressed, true);
        view.setUint32(12, uncompressed, true);
    }
    return out;
}

function centralHeader(entry: {
    name: Uint8Array;
    method: number;
    stamp: { time: number; date: number };
    crc: number;
    compressed: number;
    uncompressed: number;
    offset: number;
}): Uint8Array {
    // Only the fields that actually overflowed move into the extra, in
    // this order. A reader walks them the same way, so writing one that is
    // not needed shifts every value after it.
    const sizesWide =
        entry.uncompressed >= FIELD_MAX || entry.compressed >= FIELD_MAX;
    const offsetWide = entry.offset >= FIELD_MAX;
    const extra =
        sizesWide || offsetWide
            ? EXTRA_HEADER + (sizesWide ? 16 : 0) + (offsetWide ? 8 : 0)
            : 0;

    const out = new Uint8Array(46 + entry.name.length + extra);
    const view = new DataView(out.buffer);
    view.setUint32(0, CENTRAL_SIG, true);
    // Made by MS-DOS, at the version the contents require.
    const needs = extra > 0 ? NEEDS_ZIP64 : NEEDS_BASE;
    view.setUint16(4, needs, true);
    view.setUint16(6, needs, true);
    view.setUint16(8, FLAGS, true);
    view.setUint16(10, entry.method, true);
    view.setUint16(12, entry.stamp.time, true);
    view.setUint16(14, entry.stamp.date, true);
    view.setUint32(16, entry.crc, true);
    view.setUint32(20, sizesWide ? FIELD_MAX : entry.compressed, true);
    view.setUint32(24, sizesWide ? FIELD_MAX : entry.uncompressed, true);
    view.setUint16(28, entry.name.length, true);
    view.setUint16(30, extra, true);
    view.setUint32(42, offsetWide ? FIELD_MAX : entry.offset, true);
    out.set(entry.name, 46);

    if (extra > 0) {
        let at = 46 + entry.name.length;
        view.setUint16(at, 0x0001, true);
        view.setUint16(at + 2, extra - EXTRA_HEADER, true);
        at += EXTRA_HEADER;
        if (sizesWide) {
            wide(view, at, entry.uncompressed);
            wide(view, at + 8, entry.compressed);
            at += 16;
        }
        if (offsetWide) wide(view, at, entry.offset);
    }
    return out;
}

/// The index's own index: the records that close the archive.
///
/// Three of them when zip64 is in play — the 64-bit record, a locator
/// saying where it is, and the ordinary 22-byte record with its fields
/// saturated so an old reader knows to go looking.
function endRecords(count: number, size: number, offset: number): Uint8Array {
    const zip64 =
        count >= COUNT_MAX || size >= FIELD_MAX || offset >= FIELD_MAX;
    const out = new Uint8Array(zip64 ? 56 + 20 + 22 : 22);
    const view = new DataView(out.buffer);
    let at = 0;

    if (zip64) {
        const end64 = offset + size;
        view.setUint32(at, END64_SIG, true);
        // The record's own size, counting from the field after this one.
        wide(view, at + 4, 44);
        view.setUint16(at + 12, NEEDS_ZIP64, true);
        view.setUint16(at + 14, NEEDS_ZIP64, true);
        wide(view, at + 24, count);
        wide(view, at + 32, count);
        wide(view, at + 40, size);
        wide(view, at + 48, offset);
        at += 56;

        view.setUint32(at, LOCATOR64_SIG, true);
        wide(view, at + 8, end64);
        view.setUint32(at + 16, 1, true);
        at += 20;
    }

    view.setUint32(at, END_SIG, true);
    view.setUint16(at + 8, zip64 ? COUNT_MAX : count, true);
    view.setUint16(at + 10, zip64 ? COUNT_MAX : count, true);
    view.setUint32(at + 12, zip64 ? FIELD_MAX : size, true);
    view.setUint32(at + 16, zip64 ? FIELD_MAX : offset, true);
    return out;
}

/// The size an archive of these entries will be, near enough to plan with.
///
/// Exact for stored entries, an over-estimate for deflated ones, which is
/// the safe direction.
export function estimateSize(entries: ZipEntry[]): number {
    const encoder = new TextEncoder();
    let total = 98;
    for (const entry of entries) {
        const name = encoder.encode(normalize(entry.path)).length;
        total += 30 + name + 20 + entry.file.size + 24 + 46 + name + 24;
    }
    return total;
}

/// Writes a complete archive to `sink` and returns its length in bytes.
///
/// `onProgress` is called with the number of source bytes read so far, not
/// bytes written — it is what a progress bar should follow, since the
/// written total is unknown until deflate has finished with every entry.
export async function writeZip(
    entries: ZipEntry[],
    sink: Sink,
    onProgress?: (bytesRead: number) => void,
    signal?: AbortSignal,
): Promise<number> {
    const encoder = new TextEncoder();
    const central: Uint8Array[] = [];
    let offset = 0;
    let read = 0;

    for (const entry of entries) {
        const name = encoder.encode(normalize(entry.path));
        const method = methodFor(entry.path);
        const stamp = dosStamp(new Date(entry.file.lastModified));
        const start = offset;
        // Decided from the declared size, because the header goes out
        // before a byte has been read and the descriptor's width has to
        // match what it promised.
        const zip64 = entry.file.size >= FIELD_MAX;

        const header = localHeader(name, method, stamp, zip64);
        await sink.write(header);
        offset += header.length;

        let crc = -1;
        let compressed = 0;
        let uncompressed = 0;

        const emit = async (chunk: Uint8Array) => {
            if (chunk.length === 0) return;
            await sink.write(chunk);
            compressed += chunk.length;
            offset += chunk.length;
        };

        // `file.stream()` reads lazily off disk. `arrayBuffer()` would pull
        // the whole file into memory first, which for a 600 MB clip is the
        // very thing this module exists to avoid.
        const reader = entry.file.stream().getReader();

        if (method === STORED) {
            for (;;) {
                // Checked per chunk rather than per file: a single 600 MB
                // clip should not have to finish before a cancel lands.
                if (signal?.aborted) {
                    await reader.cancel().catch(() => undefined);
                    throw cancelled();
                }
                const { done, value } = await reader.read();
                if (done) break;
                crc = crcUpdate(crc, value);
                uncompressed += value.length;
                read += value.length;
                onProgress?.(read);
                await emit(value);
            }
        } else {
            // fflate hands its output back synchronously from inside
            // `push`, and the sink is async — so chunks are collected in
            // the callback and written once `push` has returned.
            const produced: Uint8Array[] = [];
            const deflate = new Deflate({ level: 6 }, (chunk) => {
                produced.push(chunk);
            });
            const drain = async () => {
                while (produced.length > 0) await emit(produced.shift()!);
            };

            for (;;) {
                if (signal?.aborted) {
                    await reader.cancel().catch(() => undefined);
                    throw cancelled();
                }
                const { done, value } = await reader.read();
                if (done) break;
                crc = crcUpdate(crc, value);
                uncompressed += value.length;
                read += value.length;
                onProgress?.(read);
                deflate.push(value, false);
                await drain();
            }
            deflate.push(new Uint8Array(0), true);
            await drain();
        }

        const finished = (crc ^ -1) >>> 0;
        const tail = descriptor(finished, compressed, uncompressed, zip64);
        await sink.write(tail);
        offset += tail.length;

        central.push(
            centralHeader({
                name,
                method,
                stamp,
                crc: finished,
                compressed,
                uncompressed,
                offset: start,
            }),
        );
    }

    const directoryAt = offset;
    for (const header of central) {
        await sink.write(header);
        offset += header.length;
    }

    const end = endRecords(central.length, offset - directoryAt, directoryAt);
    await sink.write(end);
    return offset + end.length;
}
