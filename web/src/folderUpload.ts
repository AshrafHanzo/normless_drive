// Turning a picked folder into something uploadable.
//
// The desktop app offers the same two choices, and they suit different
// things, so both are kept rather than picking one:
//
//   zip        one archive, structure preserved exactly, one Telegram
//              message — and the archive viewer can look inside it and
//              pull out a single file without downloading the rest
//   individual every file uploaded separately, each one downloadable on
//              its own, with its path kept in the name
//
// Which one is chosen is asked at the moment of upload, in the review
// dialog; the preference here is only what that dialog starts on.

import { zip } from "fflate";
import type { Picked, PickedFile } from "./folderPick";

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

/// Anything past this would likely exhaust the tab's memory while zipping,
/// since the whole archive is built before any of it is sent.
export const ZIP_LIMIT_BYTES = 1_500_000_000;

export function totalBytes(files: PickedFile[]): number {
    return files.reduce((sum, picked) => sum + picked.file.size, 0);
}

/// Builds a zip of the whole folder, in memory.
///
/// `mtime` is set from each file so the archive is not stamped with the
/// moment it was built — a restored folder should keep its own dates.
export async function zipFolder(
    picked: Picked,
    onProgress?: (done: number, total: number) => void,
): Promise<File> {
    const entries: Record<string, [Uint8Array, { mtime: Date }]> = {};

    for (const [index, item] of picked.files.entries()) {
        const bytes = new Uint8Array(await item.file.arrayBuffer());
        entries[item.path] = [bytes, { mtime: new Date(item.file.lastModified) }];
        onProgress?.(index + 1, picked.files.length);
    }

    const archive = await new Promise<Uint8Array>((resolve, reject) => {
        // level 6 rather than maximum: the difference on already
        // compressed content (photos, video, which is most of what goes
        // into a drive) is negligible, and the time is not.
        zip(entries, { level: 6 }, (error, data) =>
            error ? reject(error) : resolve(data),
        );
    });

    return new File([archive as BlobPart], `${picked.name}.zip`, {
        type: "application/zip",
    });
}

/// Individual mode: the path inside the folder becomes the file name, the
/// same convention the desktop app's backup uses, so a nested tree survives
/// in a flat Telegram channel and is rebuildable from the names alone.
export function flatten(picked: Picked): File[] {
    return picked.files.map(
        (item) =>
            new File([item.file], item.path, {
                type: item.file.type,
                lastModified: item.file.lastModified,
            }),
    );
}
