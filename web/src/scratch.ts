// A place to build a file that is too big to hold in memory.
//
// The origin private file system is a sandboxed directory the browser gives
// each site, invisible in the user's own file manager and never shown to
// them. Writing an archive there and reading it back as a `File` means a
// 1.9 GB zip costs one chunk of memory instead of 1.9 GB of it, and the
// existing upload path takes the result unchanged.
//
// Nothing is left behind: each run gets its own directory, removed in a
// `finally` whether the upload succeeded or not, and `sweep` clears
// anything a crashed tab abandoned.

import type { Sink } from "./zipStream";

const ROOT = "nd-scratch";

/// How long an abandoned run is left alone before it is swept.
///
/// Long enough that a tab still working on one is never robbed of it —
/// another tab cannot tell the difference between in progress and dead.
const ABANDONED_AFTER = 24 * 60 * 60 * 1000;

// Declared here rather than taken from lib.dom, which ships only part of
// this API — the same reason folderPick.ts declares its own.
interface Writable {
    write(data: Uint8Array | Blob): Promise<void>;
    close(): Promise<void>;
    abort(): Promise<void>;
}
interface FileHandle {
    getFile(): Promise<File>;
    createWritable(options?: { keepExistingData?: boolean }): Promise<Writable>;
}
interface DirectoryHandle {
    getFileHandle(name: string, options?: { create?: boolean }): Promise<FileHandle>;
    getDirectoryHandle(
        name: string,
        options?: { create?: boolean },
    ): Promise<DirectoryHandle>;
    removeEntry(name: string, options?: { recursive?: boolean }): Promise<void>;
    keys(): AsyncIterableIterator<string>;
}
interface Storage {
    getDirectory?: () => Promise<DirectoryHandle>;
    estimate?: () => Promise<{ quota?: number; usage?: number }>;
}

function storage(): Storage | null {
    const candidate = (navigator as unknown as { storage?: Storage }).storage;
    return candidate && typeof candidate.getDirectory === "function"
        ? candidate
        : null;
}

export function supportsScratch(): boolean {
    return storage() !== null;
}

export class NoRoom extends Error {
    constructor(needed: number, available: number) {
        super(
            `There is not enough room to build the archive — it needs about ${gb(needed)} and the browser will only allow ${gb(available)}. Uploading the files separately needs none.`,
        );
        this.name = "NoRoom";
    }
}

function gb(bytes: number): string {
    return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
}

/// Checks the browser will let us write this much before anything starts.
///
/// A quota it declines to report is treated as "go ahead": refusing on a
/// missing number would block a browser that is perfectly able.
export async function reserve(bytes: number): Promise<void> {
    const store = storage();
    if (!store?.estimate) return;

    const { quota, usage } = await store.estimate();
    if (!quota) return;

    // A tenth over, so a tight fit does not fail at the last entry.
    const needed = bytes * 1.1;
    const free = quota - (usage ?? 0);
    if (free < needed) throw new NoRoom(needed, free);
}

export interface Scratch {
    sink: Sink;
    /// Closes the file and hands it back under the name it was opened with.
    finish(): Promise<File>;
    /// Removes it, finished or not. Safe to call twice.
    discard(): Promise<void>;
}

/// Opens a scratch file that will be read back as `name`.
///
/// The name is the handle's own, so `getFile()` returns a `File` already
/// called the right thing — wrapping it in a renamed `File` would copy the
/// whole archive a second time.
export async function openScratch(name: string): Promise<Scratch> {
    const store = storage();
    if (!store?.getDirectory) {
        throw new Error("This browser cannot build an archive on disk.");
    }

    const base = await (await store.getDirectory()).getDirectoryHandle(ROOT, {
        create: true,
    });
    const key = `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
    const run = await base.getDirectoryHandle(key, { create: true });
    const handle = await run.getFileHandle(safeName(name), { create: true });
    const writable = await handle.createWritable();

    let open = true;
    let gone = false;

    return {
        sink: {
            write: (bytes) => writable.write(bytes),
        },
        async finish() {
            if (open) {
                await writable.close();
                open = false;
            }
            return handle.getFile();
        },
        async discard() {
            if (gone) return;
            gone = true;
            if (open) {
                open = false;
                await writable.abort().catch(() => undefined);
            }
            await base.removeEntry(key, { recursive: true }).catch(() => undefined);
        },
    };
}

/// Clears runs left behind by a tab that was closed mid-upload.
///
/// Called once at startup and allowed to fail quietly: a stale scratch file
/// is wasted disk, not a fault worth interrupting anyone over.
export async function sweep(): Promise<void> {
    const store = storage();
    if (!store?.getDirectory) return;

    try {
        const root = await store.getDirectory();
        const base = await root.getDirectoryHandle(ROOT, { create: false });
        const cutoff = Date.now() - ABANDONED_AFTER;
        for await (const key of base.keys()) {
            const made = Number.parseInt(key.split("-")[0], 36);
            if (Number.isFinite(made) && made < cutoff) {
                await base.removeEntry(key, { recursive: true }).catch(() => undefined);
            }
        }
    } catch {
        // No scratch directory yet, or no permission to look. Either way
        // there is nothing to clean.
    }
}

/// Keeps a picked folder's name usable as a file name.
function safeName(name: string): string {
    const cleaned = name.replace(/[\\/:*?"<>|\u0000-\u001f]/g, "_").trim();
    return cleaned.length > 0 ? cleaned.slice(0, 180) : "archive.zip";
}
