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
// Zip64 is deliberately absent. Telegram takes at most 2 GB in one file, so
// an archive that would need a 64-bit field could never be uploaded anyway
// — the planner splits a folder into parts below that ceiling, and the
// guards here are a backstop that fails loudly rather than writing an
// archive no reader could open.

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

/// The largest value a 32-bit zip field can hold. Anything at or above it
/// would need zip64.
const FIELD_MAX = 0xffffffff;
const ENTRY_MAX = 0xffff;

export class ZipTooLarge extends Error {
    constructor(what: string) {
        super(
            `This archive is too large to write without zip64 (${what}). It has to be split into smaller parts.`,
        );
        this.name = "ZipTooLarge";
    }
}

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

function localHeader(
    name: Uint8Array,
    method: number,
    stamp: { time: number; date: number },
): Uint8Array {
    const out = new Uint8Array(30 + name.length);
    const view = new DataView(out.buffer);
    view.setUint32(0, LOCAL_SIG, true);
    view.setUint16(4, 20, true);
    view.setUint16(6, FLAGS, true);
    view.setUint16(8, method, true);
    view.setUint16(10, stamp.time, true);
    view.setUint16(12, stamp.date, true);
    // CRC and both sizes stay zero here: they are not known yet, and the
    // descriptor written after the data carries the real values.
    view.setUint16(26, name.length, true);
    out.set(name, 30);
    return out;
}

function descriptor(
    crc: number,
    compressed: number,
    uncompressed: number,
): Uint8Array {
    const out = new Uint8Array(16);
    const view = new DataView(out.buffer);
    view.setUint32(0, DESCRIPTOR_SIG, true);
    view.setUint32(4, crc, true);
    view.setUint32(8, compressed, true);
    view.setUint32(12, uncompressed, true);
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
    const out = new Uint8Array(46 + entry.name.length);
    const view = new DataView(out.buffer);
    view.setUint32(0, CENTRAL_SIG, true);
    // Made by MS-DOS, version 2.0 — the plainest thing every reader
    // accepts, and true: nothing here uses a later feature.
    view.setUint16(4, 20, true);
    view.setUint16(6, 20, true);
    view.setUint16(8, FLAGS, true);
    view.setUint16(10, entry.method, true);
    view.setUint16(12, entry.stamp.time, true);
    view.setUint16(14, entry.stamp.date, true);
    view.setUint32(16, entry.crc, true);
    view.setUint32(20, entry.compressed, true);
    view.setUint32(24, entry.uncompressed, true);
    view.setUint16(28, entry.name.length, true);
    view.setUint32(42, entry.offset, true);
    out.set(entry.name, 46);
    return out;
}

function endRecord(
    count: number,
    size: number,
    offset: number,
): Uint8Array {
    const out = new Uint8Array(22);
    const view = new DataView(out.buffer);
    view.setUint32(0, END_SIG, true);
    view.setUint16(8, count, true);
    view.setUint16(10, count, true);
    view.setUint32(12, size, true);
    view.setUint32(16, offset, true);
    return out;
}

/// The size an archive of these entries will be, near enough to plan with.
///
/// Exact for stored entries, an over-estimate for deflated ones, which is
/// the safe direction: the planner uses it to stay under Telegram's
/// ceiling.
export function estimateSize(entries: ZipEntry[]): number {
    let total = 22;
    for (const entry of entries) {
        const name = new TextEncoder().encode(normalize(entry.path)).length;
        total += 30 + name + entry.file.size + 16 + 46 + name;
    }
    return total;
}

/// Zip paths use forward slashes and never start at a root.
function normalize(path: string): string {
    return path.replace(/\\/g, "/").replace(/^\/+/, "");
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
): Promise<number> {
    if (entries.length > ENTRY_MAX) {
        throw new ZipTooLarge(`${entries.length.toLocaleString()} files`);
    }

    const encoder = new TextEncoder();
    const central: Uint8Array[] = [];
    let offset = 0;
    let read = 0;

    for (const entry of entries) {
        const name = encoder.encode(normalize(entry.path));
        const method = methodFor(entry.path);
        const stamp = dosStamp(new Date(entry.file.lastModified));
        const start = offset;

        if (start > FIELD_MAX) throw new ZipTooLarge("it passes 4 GB");

        const header = localHeader(name, method, stamp);
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

        if (uncompressed > FIELD_MAX || compressed > FIELD_MAX) {
            throw new ZipTooLarge(`"${entry.path}" is over 4 GB`);
        }

        const finished = (crc ^ -1) >>> 0;
        const tail = descriptor(finished, compressed, uncompressed);
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

    const directorySize = offset - directoryAt;
    if (directoryAt > FIELD_MAX || directorySize > FIELD_MAX) {
        throw new ZipTooLarge("it passes 4 GB");
    }

    const end = endRecord(central.length, directorySize, directoryAt);
    await sink.write(end);
    return offset + end.length;
}
