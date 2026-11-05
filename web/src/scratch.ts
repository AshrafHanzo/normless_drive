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
    persist?: () => Promise<boolean>;
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

/// Thrown when the browser actually refuses a write, never in advance.
///
/// `navigator.storage.estimate()` is a guess, and asking it first was a
/// mistake: it reported a few gigabytes on a machine with 169 GB free and
/// refused a job that would have worked. So the archive is simply
/// attempted, and this says how far it got and what the browser claims to
/// allow — numbers worth having, rather than a flat refusal.
export class NoRoom extends Error {
    constructor(wrote: number, needed: number, limit: Allowance | null) {
        const claimed = limit
            ? ` This browser allows the site ${gb(limit.quota)} in total and reports ${gb(limit.usage)} already in use.`
            : "";
        super(
            `The browser ran out of room building the archive — it wrote ${gb(wrote)} of about ${gb(needed)}.${claimed} Uploading as separate files instead needs no room at all.`,
        );
        this.name = "NoRoom";
    }
}

function gb(bytes: number): string {
    return bytes >= 1_000_000_000
        ? `${(bytes / 1_000_000_000).toFixed(1)} GB`
        : `${Math.round(bytes / 1_000_000)} MB`;
}

interface Allowance {
    quota: number;
    usage: number;
}

async function allowance(): Promise<Allowance | null> {
    const store = storage();
    if (!store?.estimate) return null;
    try {
        const { quota, usage } = await store.estimate();
        return quota ? { quota, usage: usage ?? 0 } : null;
    } catch {
        return null;
    }
}

/// A browser is only out of room when it says so, in this one way.
function isQuotaFailure(error: unknown): boolean {
    return (
        error instanceof DOMException &&
        (error.name === "QuotaExceededError" || error.code === 22)
    );
}

/// Asks the browser to treat this site's storage as worth keeping.
///
/// Chrome grants it silently to a site someone uses, and a granted origin
/// gets a larger allowance and is not evicted under pressure. A refusal
/// changes nothing we would do differently, so the answer is ignored.
export async function requestPersistence(): Promise<void> {
    try {
        await storage()?.persist?.();
    } catch {
        // Not supported, or refused. Either way the archive is still
        // worth attempting.
    }
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
export async function openScratch(name: string, expected = 0): Promise<Scratch> {
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
    let written = 0;

    return {
        sink: {
            write: async (bytes) => {
                try {
                    await writable.write(bytes);
                    written += bytes.length;
                } catch (caught) {
                    if (!isQuotaFailure(caught)) throw caught;
                    throw new NoRoom(written, expected, await allowance());
                }
            },
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
