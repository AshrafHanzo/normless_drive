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

/// How often a running build says it is still alive.
const HEARTBEAT_EVERY = 10_000;

/// How quiet a run has to go before it is treated as abandoned.
///
/// A live build touches its marker every ten seconds, so a minute of
/// silence means the tab holding it is gone. Without the marker there
/// would be no way to tell a build in progress from a dead one, and the
/// only safe rule would be to wait a day — which is a day of someone's
/// disk held by an archive nobody is waiting for.
const SILENT_FOR = 60_000;

/// The fallback for runs left by a version that wrote no marker.
const ABANDONED_AFTER = 24 * 60 * 60 * 1000;

/// The file a live build keeps touching.
const ALIVE = ".alive";

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

    // Touched while the build runs, so a tab that opens later can tell
    // this run from one whose tab was closed and clear that one out.
    const beat = async () => {
        try {
            const marker = await run.getFileHandle(ALIVE, { create: true });
            const writer = await marker.createWritable();
            await writer.write(new Uint8Array(1));
            await writer.close();
        } catch {
            // A missed beat only risks an early sweep of this run, and
            // the sweep waits six beats before acting.
        }
    };
    await beat();
    const pulse = setInterval(() => void beat(), HEARTBEAT_EVERY);

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
            clearInterval(pulse);
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
/// Closing the tab stops the build where it stood — the archive lives in
/// that page's JavaScript and nothing survives it — so a 10 GB part-built
/// archive would otherwise sit on the disk. This reclaims it on the next
/// visit, within a minute of the build having gone quiet.
///
/// Called at startup and allowed to fail quietly: a stale scratch file is
/// wasted disk, not a fault worth interrupting anyone over.
export async function sweep(): Promise<void> {
    const store = storage();
    if (!store?.getDirectory) return;

    try {
        const root = await store.getDirectory();
        const base = await root.getDirectoryHandle(ROOT, { create: false });
        const now = Date.now();

        for await (const key of base.keys()) {
            if (await isAbandoned(base, key, now)) {
                await base.removeEntry(key, { recursive: true }).catch(() => undefined);
            }
        }
    } catch {
        // No scratch directory yet, or no permission to look. Either way
        // there is nothing to clean.
    }
}

async function isAbandoned(
    base: DirectoryHandle,
    key: string,
    now: number,
): Promise<boolean> {
    try {
        const run = await base.getDirectoryHandle(key, { create: false });
        const marker = await run.getFileHandle(ALIVE, { create: false });
        return now - (await marker.getFile()).lastModified > SILENT_FOR;
    } catch {
        // No marker: either a run from before they existed, or one whose
        // tab died before the first beat. Fall back to the time encoded
        // in the key, and give that benefit of the doubt.
        const made = Number.parseInt(key.split("-")[0], 36);
        return Number.isFinite(made) && now - made > ABANDONED_AFTER;
    }
}

/// Keeps a picked folder's name usable as a file name.
function safeName(name: string): string {
    const cleaned = name.replace(/[\\/:*?"<>|\u0000-\u001f]/g, "_").trim();
    return cleaned.length > 0 ? cleaned.slice(0, 180) : "archive.zip";
}
