// Turning a picked folder into something uploadable.
//
// The desktop app offers the same two choices, and they suit different
// things, so both are kept rather than picking one:
//
//   zip        one archive, structure preserved exactly, one Telegram
//              message — but you must download the whole thing to get
//              one file out of it
//   individual every file uploaded separately, each one downloadable on
//              its own, with its path kept in the name
//
// The path lives in `webkitRelativePath`, which the browser fills in for
// a directory pick and leaves empty otherwise.

import { zip } from "fflate";

const STORAGE_KEY = "nd_zip_folders";

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

/// The folder name the browser reports, taken from the first path segment.
export function folderNameOf(files: File[]): string {
    const first = files[0]?.webkitRelativePath ?? "";
    return first.split("/")[0] || "folder";
}

/// Path inside the picked folder, with the folder's own name stripped.
///
/// `webkitRelativePath` is `Photos/2026/a.jpg` for a pick of `Photos`;
/// keeping the leading `Photos/` would nest everything one level deeper
/// than the user chose.
export function relativePathOf(file: File): string {
    const full = file.webkitRelativePath;
    if (!full) return file.name;
    const cut = full.indexOf("/");
    return cut === -1 ? full : full.slice(cut + 1);
}

/// Anything past this would likely exhaust the tab's memory while zipping,
/// since the whole archive is built before any of it is sent.
export const ZIP_LIMIT_BYTES = 1_500_000_000;

export function totalBytes(files: File[]): number {
    return files.reduce((sum, file) => sum + file.size, 0);
}

/// Builds a zip of the whole folder, in memory.
///
/// `mtime` is set from each file so the archive is not stamped with the
/// moment it was built — a restored folder should keep its own dates.
export async function zipFolder(files: File[]): Promise<File> {
    const entries: Record<string, [Uint8Array, { mtime: Date }]> = {};

    for (const file of files) {
        const bytes = new Uint8Array(await file.arrayBuffer());
        entries[relativePathOf(file)] = [bytes, { mtime: new Date(file.lastModified) }];
    }

    const archive = await new Promise<Uint8Array>((resolve, reject) => {
        // level 6 rather than maximum: the difference on already
        // compressed content (photos, video, which is most of what goes
        // into a drive) is negligible, and the time is not.
        zip(entries, { level: 6 }, (error, data) =>
            error ? reject(error) : resolve(data),
        );
    });

    const name = `${folderNameOf(files)}.zip`;
    return new File([archive as BlobPart], name, { type: "application/zip" });
}
