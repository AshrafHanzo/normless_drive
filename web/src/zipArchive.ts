// Reading a zip that lives on the server, without downloading it.
//
// A zip keeps its index at the *end*: a 22-byte "end of central directory"
// record, preceded by one record per entry. So the whole table of contents
// of a 2 GB archive can be had by asking for its last few kilobytes, and one
// file out of the middle costs exactly that file. This is what the server's
// byte-range support is for.
//
// Everything here reads little-endian, which is what the format specifies
// regardless of the machine.

import { inflateSync } from "fflate";

const EOCD = 0x06054b50;
const EOCD64_LOCATOR = 0x07064b50;
const EOCD64 = 0x06064b50;
const CENTRAL = 0x02014b50;
const LOCAL = 0x04034b50;

/// The EOCD is 22 bytes plus a comment of up to 64 KiB.
const MAX_TAIL = 22 + 0xffff;

export interface ZipEntry {
  /// Full path inside the archive, with no leading slash.
  path: string;
  name: string;
  size: number;
  compressedSize: number;
  /// 0 = stored, 8 = deflate. Anything else this reader cannot expand.
  method: number;
  encrypted: boolean;
  directory: boolean;
  modified: Date | null;
  /// Where the local header sits, needed to find the data.
  offset: number;
}

export interface Archive {
  entries: ZipEntry[];
  /// Uncompressed total, which is what "this folder is 4 GB" should mean.
  size: number;
  compressedSize: number;
}

export class ZipError extends Error {}

/// A file on the server, read in pieces.
export class RemoteFile {
  /// Filled only if the server turns out to ignore `Range`, in which case
  /// the first read has the whole file anyway and refetching it per slice
  /// would be absurd.
  private whole: Uint8Array | null = null;

  constructor(
    private readonly url: string,
    readonly size: number,
  ) {}

  /// Inclusive on both ends, like the HTTP header it becomes.
  async read(start: number, end: number): Promise<Uint8Array> {
    if (this.whole) return this.whole.slice(start, end + 1);
    if (start > end) return new Uint8Array(0);

    const response = await fetch(this.url, {
      credentials: "same-origin",
      headers: { Range: `bytes=${start}-${end}` },
    });

    if (response.status === 416) {
      throw new ZipError("The server refused that part of the file.");
    }
    if (!response.ok) {
      throw new ZipError(`Could not read the archive (HTTP ${response.status}).`);
    }

    const bytes = new Uint8Array(await response.arrayBuffer());

    // 200 means the range was ignored and this is the entire file — older
    // proxies do that. Keep it rather than ask again.
    if (response.status === 200 && bytes.length !== end - start + 1) {
      this.whole = bytes;
      return bytes.slice(start, end + 1);
    }
    return bytes;
  }
}

/// Reads the index: a couple of small requests, whatever the archive's size.
export async function readArchive(file: RemoteFile): Promise<Archive> {
  if (file.size < 22) throw new ZipError("That file is too small to be a zip.");

  const tailLength = Math.min(file.size, MAX_TAIL);
  const tailStart = file.size - tailLength;
  const tail = await file.read(tailStart, file.size - 1);
  const view = new DataView(tail.buffer, tail.byteOffset, tail.byteLength);

  let eocd = -1;
  for (let i = tail.length - 22; i >= 0; i--) {
    if (view.getUint32(i, true) === EOCD) {
      eocd = i;
      break;
    }
  }
  if (eocd === -1) {
    throw new ZipError(
      "This does not look like a zip — no index was found at the end of it.",
    );
  }

  let count = view.getUint16(eocd + 10, true);
  let directorySize = view.getUint32(eocd + 12, true);
  let directoryStart = view.getUint32(eocd + 16, true);

  // Zip64. A 32-bit field saturated to all-ones means the real value lives
  // in a second record — which is how an archive of more than 65535 files
  // says so, something a folder upload reaches sooner than you would think.
  if (count === 0xffff || directorySize === 0xffffffff || directoryStart === 0xffffffff) {
    const locator = eocd - 20;
    if (locator < 0 || view.getUint32(locator, true) !== EOCD64_LOCATOR) {
      throw new ZipError("This zip needs a zip64 index that is missing.");
    }
    const at = Number(view.getBigUint64(locator + 8, true));
    const record = await file.read(at, at + 55);
    const record64 = new DataView(record.buffer, record.byteOffset, record.byteLength);
    if (record64.getUint32(0, true) !== EOCD64) {
      throw new ZipError("This zip's zip64 index is damaged.");
    }
    count = Number(record64.getBigUint64(32, true));
    directorySize = Number(record64.getBigUint64(40, true));
    directoryStart = Number(record64.getBigUint64(48, true));
  }

  // Usually already in hand: the index of a modest archive sits inside the
  // tail that was fetched to find the EOCD.
  const directory =
    directoryStart >= tailStart
      ? tail.subarray(directoryStart - tailStart, directoryStart - tailStart + directorySize)
      : await file.read(directoryStart, directoryStart + directorySize - 1);

  return parseDirectory(directory, count);
}

function parseDirectory(bytes: Uint8Array, count: number): Archive {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const entries: ZipEntry[] = [];
  let at = 0;
  let size = 0;
  let compressedSize = 0;

  while (at + 46 <= bytes.length && entries.length < count) {
    if (view.getUint32(at, true) !== CENTRAL) break;

    const flags = view.getUint16(at + 8, true);
    const method = view.getUint16(at + 10, true);
    const time = view.getUint16(at + 12, true);
    const date = view.getUint16(at + 14, true);
    const nameLength = view.getUint16(at + 28, true);
    const extraLength = view.getUint16(at + 30, true);
    const commentLength = view.getUint16(at + 32, true);
    const external = view.getUint32(at + 38, true);

    let uncompressed = view.getUint32(at + 24, true);
    let compressed = view.getUint32(at + 20, true);
    let offset = view.getUint32(at + 42, true);

    const nameAt = at + 46;
    const path = decodeName(bytes.subarray(nameAt, nameAt + nameLength), flags);
    const extra = bytes.subarray(nameAt + nameLength, nameAt + nameLength + extraLength);

    // The zip64 extra field carries only the fields that overflowed, in a
    // fixed order, so which ones are present depends on what was saturated.
    if (uncompressed === 0xffffffff || compressed === 0xffffffff || offset === 0xffffffff) {
      const wide = findZip64Extra(extra);
      if (wide) {
        let cursor = 0;
        if (uncompressed === 0xffffffff) {
          uncompressed = Number(wide.getBigUint64(cursor, true));
          cursor += 8;
        }
        if (compressed === 0xffffffff) {
          compressed = Number(wide.getBigUint64(cursor, true));
          cursor += 8;
        }
        if (offset === 0xffffffff) {
          offset = Number(wide.getBigUint64(cursor, true));
        }
      }
    }

    const directory = path.endsWith("/") || (uncompressed === 0 && (external & 0x10) !== 0);

    entries.push({
      path: path.replace(/\/+$/, ""),
      name: path.replace(/\/+$/, "").split("/").pop() ?? path,
      size: uncompressed,
      compressedSize: compressed,
      method,
      encrypted: (flags & 0x1) !== 0,
      directory,
      modified: decodeDate(date, time),
      offset,
    });

    if (!directory) {
      size += uncompressed;
      compressedSize += compressed;
    }

    at = nameAt + nameLength + extraLength + commentLength;
  }

  return { entries, size, compressedSize };
}

function findZip64Extra(extra: Uint8Array): DataView | null {
  const view = new DataView(extra.buffer, extra.byteOffset, extra.byteLength);
  let at = 0;
  while (at + 4 <= extra.length) {
    const id = view.getUint16(at, true);
    const length = view.getUint16(at + 2, true);
    if (id === 0x0001) {
      return new DataView(extra.buffer, extra.byteOffset + at + 4, length);
    }
    at += 4 + length;
  }
  return null;
}

/// Bit 11 promises UTF-8. Without it the name is officially CP437, but in
/// practice it is far more often UTF-8 written by a tool that forgot the
/// flag — so UTF-8 is tried strictly first and Windows-1252 catches the
/// genuinely old ones instead of leaving replacement characters.
function decodeName(bytes: Uint8Array, flags: number): string {
  if (flags & 0x800) return new TextDecoder("utf-8").decode(bytes);
  try {
    return new TextDecoder("utf-8", { fatal: true }).decode(bytes);
  } catch {
    return new TextDecoder("windows-1252").decode(bytes);
  }
}

/// MS-DOS packed date and time: two bytes each, two-second resolution, and
/// years counted from 1980.
function decodeDate(date: number, time: number): Date | null {
  if (date === 0) return null;
  const year = ((date >> 9) & 0x7f) + 1980;
  const month = ((date >> 5) & 0x0f) - 1;
  const day = date & 0x1f;
  const hours = (time >> 11) & 0x1f;
  const minutes = (time >> 5) & 0x3f;
  const seconds = (time & 0x1f) * 2;
  const parsed = new Date(year, month, day, hours, minutes, seconds);
  return Number.isNaN(parsed.getTime()) ? null : parsed;
}

/// Pulls one file out of the archive.
///
/// Two small requests: the local header, whose name and extra lengths say
/// where the data starts, then the data itself. The central directory's
/// copies of those lengths cannot be used — a zip is allowed to differ.
export async function extract(file: RemoteFile, entry: ZipEntry): Promise<Uint8Array> {
  if (entry.encrypted) {
    throw new ZipError("That file inside the archive is password-protected.");
  }
  if (entry.method !== 0 && entry.method !== 8) {
    throw new ZipError(
      `That file uses compression this viewer cannot read (method ${entry.method}).`,
    );
  }

  const header = await file.read(entry.offset, entry.offset + 29);
  const view = new DataView(header.buffer, header.byteOffset, header.byteLength);
  if (view.getUint32(0, true) !== LOCAL) {
    throw new ZipError("The archive's index points somewhere that is not a file.");
  }

  const start =
    entry.offset + 30 + view.getUint16(26, true) + view.getUint16(28, true);
  const packed = await file.read(start, start + entry.compressedSize - 1);

  if (entry.method === 0) return packed;

  // A zip entry is raw DEFLATE with no zlib wrapper. Handing inflate the
  // expected size lets it allocate once instead of growing.
  return inflateSync(packed, { out: new Uint8Array(entry.size) });
}
