// Turning a picked folder into something uploadable.
//
// The desktop app offers the same two choices, and they suit different
// things, so both are kept rather than picking one:
//
//   zip        the structure preserved exactly, as few Telegram messages
//              as the 2 GB per-file ceiling allows — and the archive
//              viewer can look inside one and pull out a single file
//              without downloading the rest
//   individual every file uploaded separately, each one downloadable on
//              its own, with its path kept in the name
//
// Which one is chosen is asked at the moment of upload, in the review
// dialog; the preference here is only what that dialog starts on.
//
// Zipping used to stop at 1.5 GB because the whole archive was assembled
// in the tab's memory. It is now written through to a scratch file on
// disk, so the only real ceiling is Telegram's: 2 GB in one message. A
// folder past that is split across several archives, each one complete and
// openable on its own, rather than refused.

import type { Picked, PickedFile } from "./folderPick";
import { openScratch, reserve, supportsScratch } from "./scratch";
import { estimateSize, writeZip, type Sink, type ZipEntry } from "./zipStream";

const STORAGE_KEY = "nd_zip_folders";

export type UploadMode = "zip" | "individual";

/// Matches the desktop app's default.
export function loadZipPreference(): boolean {
    try {
        const stored = localStorage.getItem(STORAGE_KEY);
        return stored === null ? true : stored === "true";
    } catch {
        return true;
    }
}

export function saveZipPreference(value: boolean) {
    try {
        localStorage.setItem(STORAGE_KEY, String(value));
    } catch {
        // A forgotten preference is a much smaller problem than refusing
        // to change it.
    }
}

/// Telegram refuses a single file above 2 GB. The margin below it covers
/// the archive's own headers — about a hundred bytes per entry — and
/// leaves room for a file that grew between being picked and being read.
export const PART_LIMIT_BYTES = 1_900_000_000;

/// The ceiling when there is no scratch file to write to and the archive
/// has to be assembled in memory after all. Lower than Telegram's, because
/// the browser, not Telegram, is what gives out first.
export const MEMORY_PART_LIMIT_BYTES = 500_000_000;

/// A zip's central directory counts entries in sixteen bits.
const MAX_ENTRIES = 65_535;

export function totalBytes(files: PickedFile[]): number {
    return files.reduce((sum, picked) => sum + picked.file.size, 0);
}

/// One archive's worth of the folder.
export interface ZipPart {
    name: string;
    entries: PickedFile[];
    bytes: number;
}

export interface ZipPlan {
    parts: ZipPart[];
    /// Files no archive can hold, because one of them alone is over the
    /// ceiling. They go up on their own instead of blocking the rest.
    oversized: PickedFile[];
    /// Whether the archives are built on disk or in memory, which is what
    /// sets the ceiling they were packed against.
    streamed: boolean;
    limit: number;
}

/// Decides how many archives the folder needs and what goes in each.
///
/// Packed in the order the files were found rather than by size: an
/// archive whose contents are a contiguous slice of the folder is one
/// someone can reason about, and a few per cent of wasted space is a fair
/// price for that.
export function planZip(picked: Picked, streamed = supportsScratch()): ZipPlan {
    const limit = streamed ? PART_LIMIT_BYTES : MEMORY_PART_LIMIT_BYTES;
    const oversized: PickedFile[] = [];
    const bins: PickedFile[][] = [];

    let current: PickedFile[] = [];
    let currentBytes = 0;

    for (const item of picked.files) {
        if (item.file.size > limit) {
            oversized.push(item);
            continue;
        }
        const wouldOverflow = currentBytes + item.file.size > limit;
        if (current.length > 0 && (wouldOverflow || current.length >= MAX_ENTRIES)) {
            bins.push(current);
            current = [];
            currentBytes = 0;
        }
        current.push(item);
        currentBytes += item.file.size;
    }
    if (current.length > 0) bins.push(current);

    const parts = bins.map((entries, index) => ({
        name:
            bins.length === 1
                ? `${picked.name}.zip`
                : `${picked.name} (${index + 1} of ${bins.length}).zip`,
        entries,
        bytes: totalBytes(entries),
    }));

    return { parts, oversized, streamed, limit };
}

export interface BuiltArchive {
    file: File;
    /// Frees the scratch file. Always call it, including after a failure.
    release(): Promise<void>;
}

/// Builds one archive and hands back something the uploader can send.
///
/// On disk where the browser allows it, in memory where it does not — the
/// writer is the same either way, only the sink differs.
export async function buildArchive(
    part: ZipPart,
    streamed: boolean,
    onProgress?: (bytesRead: number, bytesTotal: number) => void,
): Promise<BuiltArchive> {
    const entries: ZipEntry[] = part.entries.map((item) => ({
        path: item.path,
        file: item.file,
    }));
    const report = onProgress
        ? (read: number) => onProgress(read, part.bytes)
        : undefined;

    if (!streamed) {
        const chunks: Uint8Array[] = [];
        const sink: Sink = {
            write: async (bytes) => {
                // Copied: a sink that keeps the caller's view would hold a
                // reference into a buffer the writer may reuse.
                chunks.push(bytes.slice());
            },
        };
        await writeZip(entries, sink, report);
        const file = new File(chunks as BlobPart[], part.name, {
            type: "application/zip",
        });
        return { file, release: async () => undefined };
    }

    await reserve(estimateSize(entries));
    const scratch = await openScratch(part.name);
    try {
        await writeZip(entries, scratch.sink, report);
        return { file: await scratch.finish(), release: () => scratch.discard() };
    } catch (caught) {
        await scratch.discard();
        throw caught;
    }
}

/// Individual mode: the path inside the folder becomes the file name, the
/// same convention the desktop app's backup uses, so a nested tree survives
/// in a flat Telegram channel and is rebuildable from the names alone.
export function flatten(picked: Picked): File[] {
    return picked.files.map(named);
}

export function named(item: PickedFile): File {
    return new File([item.file], item.path, {
        type: item.file.type,
        lastModified: item.file.lastModified,
    });
}
