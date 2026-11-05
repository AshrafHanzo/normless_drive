import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  FileArchive,
  FileAudio,
  FileImage,
  FileText,
  FileVideo,
  File as FileIcon,
  FolderInput,
  FolderOpen,
  FolderUp,
  Globe,
  Inbox,
  LayoutGrid,
  List,
  LogOut,
  RotateCw,
  Search,
  Settings,
  Share2,
  Shield,
  Trash2,
  Upload,
} from "lucide-react";
import {
  api,
  ApiError,
  type FileRow,
  type Folder,
  type Group,
  type User,
} from "../api";
import { Alert, Button, Spinner } from "../ui";
import { Preferences } from "./Preferences";
import { RemoteUpload } from "./RemoteUpload";
import { NewFolder } from "./NewFolder";
import { NewGroup } from "./NewGroup";
import { Sidebar } from "./Sidebar";
import { UploadQueue, type QueueItem } from "./UploadQueue";
import {
  buildArchive,
  flatten,
  named,
  planZip,
  type UploadMode,
} from "../folderUpload";
import { supportsScratch, sweep } from "../scratch";
import {
  fromInput,
  pickDirectory,
  supportsDirectoryHandle,
  type Picked,
} from "../folderPick";
import { formatSize } from "../paths";
import { FolderReview } from "./FolderReview";
import { MoveDialog } from "./MoveDialog";
import { ShareDialog } from "./ShareDialog";
import { ZipViewer } from "./ZipViewer";

type View = "list" | "grid";

export function Drive({
  user,
  phoneHint,
  onDisconnected,
  onSignOut,
  onOpenAdmin,
}: {
  user: User;
  phoneHint: string | null;
  onDisconnected: () => void;
  onSignOut: () => void;
  onOpenAdmin: () => void;
}) {
  const [files, setFiles] = useState<FileRow[] | null>(null);
  const [folders, setFolders] = useState<Folder[]>([]);
  const [folder, setFolder] = useState<string | undefined>(undefined);
  const [query, setQuery] = useState("");
  const [view, setView] = useState<View>("list");
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [sharing, setSharing] = useState(false);
  const [moving, setMoving] = useState(false);
  const [prefs, setPrefs] = useState(false);
  const [remote, setRemote] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [newFolder, setNewFolder] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [newGroup, setNewGroup] = useState(false);
  const [queue, setQueue] = useState<QueueItem[]>([]);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [picked, setPicked] = useState<Picked | null>(null);
  const [picking, setPicking] = useState(false);
  const [viewing, setViewing] = useState<FileRow | null>(null);
  const [groups, setGroups] = useState<Group[]>([]);
  const picker = useRef<HTMLInputElement>(null);
  const folderPicker = useRef<HTMLInputElement>(null);
  const searchBox = useRef<HTMLInputElement>(null);

  // The upload queue's machinery. `chain` is the single worker: every
  // batch is appended to it, so two of them can never run at once.
  // `pending` counts the jobs still on it, and `nextId` keeps queue rows
  // unique across batches.
  const chain = useRef<Promise<void>>(Promise.resolve());
  const pending = useRef(0);
  const nextId = useRef(0);
  // Read by a job that finishes long after it started, when the folder
  // on screen may no longer be the one it was uploading into.
  const current = useRef(folder);
  current.current = folder;

  const load = useCallback(async (which?: string) => {
    setError(null);
    setFiles(null);
    try {
      setFiles((await api.files.list(which)).files);
    } catch (caught) {
      setError(caught instanceof ApiError ? caught.message : "Could not load files.");
      setFiles([]);
    }
  }, []);

  const loadFolders = useCallback(async () => {
    try {
      setFolders((await api.folders.list()).folders);
      setGroups((await api.groups.list()).groups);
    } catch {
      // Saved Messages still works without the folder list, so a failure
      // here must not blank the page.
    }
  }, []);

  useEffect(() => void load(folder), [load, folder]);
  useEffect(() => void loadFolders(), [loadFolders]);

  // Archives are built in a private scratch directory. A tab closed
  // mid-upload leaves one behind, so yesterday's are cleared on the way
  // in — quietly, since a stale scratch file is wasted disk and not a
  // fault anyone needs telling about.
  useEffect(() => void sweep(), []);

  // Ctrl/Cmd+F focuses search, the same shortcut the desktop app shows.
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key === "f") {
        event.preventDefault();
        searchBox.current?.focus();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  /// Moves one queue row along. By id, never by index — rows from a
  /// second batch sit after the first in the same list, and an index into
  /// one batch means something quite different in the other.
  const mark = useCallback(
    (id: string, state: QueueItem["state"], error?: string) => {
      setQueue((rows) =>
        rows.map((row) => (row.id === id ? { ...row, state, error } : row)),
      );
    },
    [],
  );

  /// Adds a batch to the one upload queue.
  ///
  /// There is a single worker, and everything waits its turn on it.
  /// Uploads used to run as a loop per batch, so starting a second batch
  /// — dropping files while the first was still going — ran two loops at
  /// once against the same Telegram connection, each writing over the
  /// other's rows. The first batch looked like it had stopped. It is one
  /// chain now, so a second batch simply queues behind the first.
  ///
  /// Sequential within a batch too: every byte crosses this server on its
  /// way to Telegram, so running them in parallel makes nothing faster —
  /// only each one slower and the progress harder to read.
  const enqueue = useCallback(
    (rows: QueueItem[], work: () => Promise<void>) => {
      if (rows.length === 0) return;
      setError(null);
      setQueue((existing) => [...existing, ...rows]);
      pending.current += 1;
      setUploading(true);

      chain.current = chain.current
        .then(work)
        .catch(() => undefined)
        .then(async () => {
          pending.current -= 1;
          if (pending.current === 0) setUploading(false);
          await load(current.current);
        });
    },
    [load],
  );

  function rowsFor(files: { name: string; size: number }[]): QueueItem[] {
    return files.map((file) => {
      nextId.current += 1;
      return {
        id: `q${nextId.current}`,
        name: file.name,
        size: file.size,
        state: "waiting" as const,
      };
    });
  }

  function send(files: File[]) {
    if (files.length === 0) return;
    // Captured now: the queue may still be running when someone switches
    // folders, and these files belong where they were dropped.
    const target = folder;
    const rows = rowsFor(files);

    enqueue(rows, async () => {
      for (const [index, file] of files.entries()) {
        const id = rows[index].id;
        mark(id, "uploading");
        try {
          await api.files.upload(file, target);
          mark(id, "done");
        } catch (caught) {
          mark(
            id,
            "failed",
            caught instanceof ApiError ? caught.message : "Upload failed.",
          );
        }
      }
    });
  }

  /// Pulls in folders that exist in Telegram but not here — the ones made
  /// by the desktop app, which uses the same channel naming.
  function toggle(id: number) {
    setSelected((current) => {
      const next = new Set(current);
      if (!next.delete(id)) next.add(id);
      return next;
    });
  }

  async function deleteSelected() {
    const ids = [...selected];
    if (
      !window.confirm(
        `Delete ${ids.length} file${ids.length === 1 ? "" : "s"}? This removes them from Telegram permanently.`,
      )
    ) {
      return;
    }
    setError(null);
    for (const id of ids) {
      await api.files.remove(id, folder).catch(() => undefined);
    }
    setSelected(new Set());
    await load(folder);
  }

  /// Opens the folder picker and reads the tree, without sending anything.
  ///
  /// Where the File System Access API exists the browser asks to *view* the
  /// folder — one calm question — instead of "Upload 57 files to this
  /// site?". Everywhere else the hidden input is used and that prompt is
  /// the browser's to show, not ours to style. Either way the decision to
  /// upload is taken afterwards, in the review dialog.
  async function openFolder() {
    setError(null);
    if (!supportsDirectoryHandle()) {
      folderPicker.current?.click();
      return;
    }

    setPicking(true);
    try {
      const result = await pickDirectory((count) =>
        setProgress(`Reading the folder… ${count.toLocaleString()} files so far`),
      );
      if (result && result.files.length > 0) setPicked(result);
      else if (result) setError("That folder has no files in it.");
    } catch (caught) {
      setError(
        caught instanceof Error ? caught.message : "Could not read that folder.",
      );
    } finally {
      setPicking(false);
      setProgress(null);
    }
  }

  /// Uploads a reviewed directory, either as archives or file by file.
  function sendFolder(chosen: Picked, mode: UploadMode) {
    setPicked(null);
    if (folderPicker.current) folderPicker.current.value = "";

    if (mode === "individual") {
      send(flatten(chosen));
      return;
    }

    const target = folder;
    const streamed = supportsScratch();
    const plan = planZip(chosen, streamed);
    const rows = rowsFor(
      plan.parts.map((part) => ({ name: part.name, size: part.bytes })),
    );

    enqueue(rows, async () => {
      for (const [index, part] of plan.parts.entries()) {
        const id = rows[index].id;
        mark(id, "uploading");
        let built = null as Awaited<ReturnType<typeof buildArchive>> | null;
        try {
          // Each archive is built, sent and thrown away before the next
          // one starts, so a four-part upload never needs room for more
          // than one part at a time.
          let shown = -1;
          built = await buildArchive(part, streamed, (read, total) => {
            // Throttled to whole per cent: a render per chunk would be
            // thousands of them saying nothing new.
            const percent = total > 0 ? Math.floor((read / total) * 100) : 0;
            if (percent === shown) return;
            shown = percent;
            setProgress(
              plan.parts.length > 1
                ? `Packing ${part.name} — ${percent}% (${index + 1} of ${plan.parts.length})`
                : `Packing ${part.name} — ${percent}%`,
            );
          });
          setProgress(null);
          await api.files.upload(built.file, target);
          mark(id, "done");
        } catch (caught) {
          mark(
            id,
            "failed",
            caught instanceof Error ? caught.message : "Could not build that archive.",
          );
        } finally {
          setProgress(null);
          // The scratch file goes whether the upload worked or not —
          // leaving a 1.9 GB archive behind on a failure would be worse
          // than the failure.
          await built?.release();
        }
      }
    });

    // Anything too large for an archive of its own still has to go
    // somewhere, so it follows behind as an ordinary file.
    if (plan.oversized.length > 0) send(plan.oversized.map(named));
  }

  async function syncFolders() {
    setSyncing(true);
    setError(null);
    try {
      const result = await api.folders.sync();
      await loadFolders();
      setProgress(
        result.added > 0
          ? `Found ${result.added} folder${result.added === 1 ? "" : "s"} from Telegram.`
          : "No new folders — everything is already here.",
      );
      window.setTimeout(() => setProgress(null), 4000);
    } catch (caught) {
      setError(caught instanceof ApiError ? caught.message : "Sync failed.");
    } finally {
      setSyncing(false);
    }
  }

  const activeName =
    folder === undefined
      ? "Saved Messages"
      : (folders.find((f) => f.id === folder)?.name ?? "Folder");

  // Filtering client-side: the list is already capped at 500, so a round
  // trip per keystroke would be slower and no more accurate.
  const shown = useMemo(() => {
    if (!files) return null;
    const needle = query.trim().toLowerCase();
    return needle
      ? files.filter((f) => f.name.toLowerCase().includes(needle))
      : files;
  }, [files, query]);

  const totalBytes = useMemo(
    () => (files ?? []).reduce((sum, f) => sum + f.size, 0),
    [files],
  );

  return (
    <div
      className="flex h-full bg-canvas"
      onDragOver={(e) => {
        e.preventDefault();
        setDragging(true);
      }}
      onDragLeave={() => setDragging(false)}
      onDrop={(e) => {
        e.preventDefault();
        setDragging(false);
        const dropped = Array.from(e.dataTransfer.files ?? []);
        if (dropped.length) send(dropped);
      }}
    >
      {sidebarOpen && (
        <Sidebar
          folders={folders}
          groups={groups}
          selected={folder}
          phoneHint={phoneHint}
          totalBytes={formatSize(totalBytes)}
          creating={false}
          syncing={syncing}
          onSelect={setFolder}
          onNewFolder={() => setNewFolder(true)}
          onNewGroup={() => setNewGroup(true)}
          onSync={() => void syncFolders()}
          onDisconnect={async () => {
            await api.telegram.disconnect().catch(() => undefined);
            onDisconnected();
          }}
          onCollapse={() => setSidebarOpen(false)}
          onChanged={() => void loadFolders()}
        />
      )}
      {/* ── Main ────────────────────────────────────────────────── */}
      <div className="flex min-w-0 flex-1 flex-col">
        <header className="flex items-center gap-3 border-b border-border px-4 py-3">
          {!sidebarOpen && (
            <button
              onClick={() => setSidebarOpen(true)}
              aria-label="Show sidebar"
              className="text-ink-faint transition-colors hover:text-ink"
            >
              <List className="h-4 w-4" />
            </button>
          )}

          <h1 className="shrink-0 text-base font-semibold text-ink">{activeName}</h1>

          <div className="relative mx-auto w-full max-w-md">
            <Search className="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-ink-faint" />
            <input
              ref={searchBox}
              value={query}
              onChange={(e) => setQuery(e.target.value)}
              placeholder="Search files…"
              className="w-full rounded-full border border-border bg-canvas py-1.5 pl-9 pr-3
                         text-sm text-ink outline-none transition-colors
                         placeholder:text-ink-faint focus:border-accent"
            />
          </div>

          <div className="flex shrink-0 items-center gap-1.5">
            <IconButton
              icon={view === "list" ? LayoutGrid : List}
              label={view === "list" ? "Grid view" : "List view"}
              onClick={() => setView(view === "list" ? "grid" : "list")}
            />
            <IconButton icon={Globe} label="Upload from a URL" onClick={() => setRemote(true)} />
            <IconButton icon={Share2} label="Share this folder" onClick={() => setSharing(true)} />
            <IconButton icon={RotateCw} label="Refresh" onClick={() => void load(folder)} />
            <IconButton icon={Settings} label="Preferences" onClick={() => setPrefs(true)} />
            {user.role === "admin" && (
              <IconButton icon={Shield} label="Administration" onClick={onOpenAdmin} />
            )}
            <IconButton icon={LogOut} label="Sign out" onClick={onSignOut} />

            {/* Not disabled while an upload runs: the queue takes a new
                batch and runs it after the current one, which is the
                whole point of there being a queue. */}
            <Button
              variant="ghost"
              busy={picking}
              onClick={() => void openFolder()}
              title="Upload a whole folder"
            >
              <FolderUp className="h-3.5 w-3.5" />
              Folder
            </Button>
            <Button onClick={() => picker.current?.click()}>
              <Upload className="h-3.5 w-3.5" />
              Upload
            </Button>
            <input
              ref={picker}
              type="file"
              // Without this the browser's picker allows exactly one file,
              // whatever the change handler is prepared to accept.
              multiple
              hidden
              onChange={(e) => {
                const chosen = Array.from(e.target.files ?? []);
                // Cleared straight away, not after the upload: the same
                // files picked twice in a row must still fire a change.
                e.target.value = "";
                if (chosen.length) send(chosen);
              }}
            />
            <input
              ref={folderPicker}
              type="file"
              // The fallback for Firefox and Safari, which have no File
              // System Access API. Non-standard, but the only way those
              // browsers will offer a directory picker at all.
              // @ts-expect-error -- not in React's typings
              webkitdirectory=""
              directory=""
              multiple
              hidden
              onChange={(e) => {
                const chosen = fromInput(e.target.files);
                if (chosen) setPicked(chosen);
              }}
            />
          </div>
        </header>

        <main className="relative flex-1 overflow-y-auto p-4">
          {dragging && (
            <div className="pointer-events-none absolute inset-4 z-10 flex items-center justify-center rounded-[var(--radius-card)] border-2 border-dashed border-accent bg-accent-soft/70">
              <p className="text-sm font-medium text-accent">Drop to upload</p>
            </div>
          )}

          {error && (
            <div className="mb-3">
              <Alert tone="error">{error}</Alert>
            </div>
          )}
          {progress && (
            <div className="mb-3">
              <Alert tone="warn">{progress}</Alert>
            </div>
          )}

          {/* Only appears once something is picked, so it costs nothing
              the rest of the time. */}
          {selected.size > 0 && (
            <div className="mb-3 flex flex-wrap items-center gap-2 rounded-[var(--radius-card)] border border-accent/40 bg-accent-soft px-3 py-2">
              <span className="text-sm font-medium text-accent">
                {selected.size} selected
              </span>
              <div className="ml-auto flex flex-wrap gap-1.5">
                <Button variant="ghost" onClick={() => setSharing(true)}>
                  <Share2 className="h-3.5 w-3.5" />
                  Share these
                </Button>
                <Button variant="ghost" onClick={() => setMoving(true)}>
                  <FolderInput className="h-3.5 w-3.5" />
                  Move to…
                </Button>
                <Button variant="ghost" onClick={() => void deleteSelected()}>
                  <Trash2 className="h-3.5 w-3.5" />
                  Delete
                </Button>
                <Button variant="ghost" onClick={() => setSelected(new Set())}>
                  Clear
                </Button>
              </div>
            </div>
          )}

          {shown === null ? (
            <Spinner />
          ) : shown.length === 0 ? (
            <Empty
              searching={query.trim().length > 0}
              onUpload={() => picker.current?.click()}
            />
          ) : view === "list" ? (
            <div className="overflow-hidden rounded-[var(--radius-card)] border border-border bg-raised">
              <div className="flex items-center gap-3 border-b border-border px-4 py-2">
                <input
                  type="checkbox"
                  aria-label="Select all files"
                  checked={shown.length > 0 && selected.size === shown.length}
                  onChange={(e) =>
                    setSelected(
                      e.target.checked
                        ? new Set(shown.map((f) => f.id))
                        : new Set(),
                    )
                  }
                  className="h-3.5 w-3.5 accent-[var(--color-accent)]"
                />
                <span className="text-xs text-ink-faint">
                  {selected.size > 0
                    ? `${selected.size} of ${shown.length}`
                    : "Select all"}
                </span>
              </div>
              {shown.map((file) => (
                <ListRow
                  key={file.id}
                  file={file}
                  folder={folder}
                  checked={selected.has(file.id)}
                  onToggle={() => toggle(file.id)}
                  onOpen={isArchive(file) ? () => setViewing(file) : undefined}
                  onDeleted={() => void load(folder)}
                />
              ))}
            </div>
          ) : (
            <div className="grid grid-cols-[repeat(auto-fill,minmax(160px,1fr))] gap-3">
              {shown.map((file) => (
                <GridCard
                  key={file.id}
                  file={file}
                  folder={folder}
                  onOpen={isArchive(file) ? () => setViewing(file) : undefined}
                  onDeleted={() => void load(folder)}
                />
              ))}
            </div>
          )}

          {shown && shown.length > 0 && (
            <p className="mt-3 text-center text-xs text-ink-faint">
              {shown.length} of {files?.length ?? 0} file
              {(files?.length ?? 0) === 1 ? "" : "s"} · {formatSize(totalBytes)}
            </p>
          )}
        </main>
      </div>

      {sharing && (
        <ShareDialog
          folderId={folder}
          folderName={activeName}
          messageIds={[...selected]}
          onClose={() => setSharing(false)}
          onFolderCreated={(id) => {
            void loadFolders();
            setFolder(id);
          }}
        />
      )}
      {moving && (
        <MoveDialog
          ids={[...selected]}
          currentFolder={folder}
          folders={folders}
          onClose={() => setMoving(false)}
          onMoved={(summary) => {
            setMoving(false);
            setSelected(new Set());
            setProgress(summary);
            window.setTimeout(() => setProgress(null), 5000);
            // Folders too: the destination may have been created inside
            // the dialog, and the sidebar's sizes have changed either way.
            void loadFolders();
            void load(folder);
          }}
        />
      )}
      {picked && (
        <FolderReview
          picked={picked}
          busy={uploading}
          onCancel={() => {
            setPicked(null);
            if (folderPicker.current) folderPicker.current.value = "";
          }}
          onConfirm={(mode) => sendFolder(picked, mode)}
        />
      )}
      {viewing && (
        <ZipViewer
          url={api.files.downloadUrl(viewing.id, folder)}
          name={viewing.name}
          size={viewing.size}
          onClose={() => setViewing(null)}
        />
      )}
      {prefs && <Preferences phoneHint={phoneHint} onClose={() => setPrefs(false)} />}
      <UploadQueue items={queue} onDismiss={() => setQueue([])} />

      {newGroup && (
        <NewGroup
          onClose={() => setNewGroup(false)}
          onCreated={() => {
            setNewGroup(false);
            void loadFolders();
          }}
        />
      )}
      {newFolder && (
        <NewFolder
          onClose={() => setNewFolder(false)}
          onCreated={(id) => {
            setNewFolder(false);
            void loadFolders();
            setFolder(id);
          }}
        />
      )}
      {remote && (
        <RemoteUpload
          folder={folder}
          onClose={() => setRemote(false)}
          onUploaded={() => {
            setRemote(false);
            void load(folder);
          }}
        />
      )}
    </div>
  );
}

function IconButton({
  icon: Icon,
  label,
  onClick,
}: {
  icon: typeof Search;
  label: string;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      title={label}
      aria-label={label}
      className="rounded-[var(--radius-control)] p-2 text-ink-faint transition-colors
                 hover:bg-surface hover:text-ink"
    >
      <Icon className="h-4 w-4" />
    </button>
  );
}

function Empty({
  searching,
  onUpload,
}: {
  searching: boolean;
  onUpload: () => void;
}) {
  return (
    <div className="flex flex-col items-center justify-center rounded-[var(--radius-card)] border border-border bg-raised px-6 py-20 text-center">
      <div className="mb-4 flex h-14 w-14 items-center justify-center rounded-full bg-accent-soft">
        <Inbox className="h-6 w-6 text-accent" />
      </div>
      <p className="text-base font-semibold text-ink">
        {searching ? "No files match" : "This folder is empty"}
      </p>
      <p className="mx-auto mt-1.5 max-w-sm text-sm leading-relaxed text-ink-muted">
        {searching
          ? "Try a different search term."
          : "Drag and drop files here, or use the button below. They go straight to your own Telegram."}
      </p>
      {!searching && (
        <Button onClick={onUpload} className="mt-5">
          <Upload className="h-3.5 w-3.5" />
          Upload a file
        </Button>
      )}
      <p className="mt-5 text-xs text-ink-faint">
        Tip: press{" "}
        <kbd className="rounded border border-border bg-surface px-1.5 py-0.5 font-mono text-[11px]">
          Ctrl / Cmd + F
        </kbd>{" "}
        to search
      </p>
    </div>
  );
}

function useDelete(file: FileRow, folder: string | undefined, onDeleted: () => void) {
  const [deleting, setDeleting] = useState(false);
  const remove = async () => {
    // Telegram has no undo, so this asks first.
    if (
      !window.confirm(
        `Delete "${file.name}"? This removes it from Telegram permanently.`,
      )
    ) {
      return;
    }
    setDeleting(true);
    try {
      await api.files.remove(file.id, folder);
      onDeleted();
    } catch {
      setDeleting(false);
    }
  };
  return { deleting, remove };
}

function ListRow({
  file,
  folder,
  checked,
  onToggle,
  onOpen,
  onDeleted,
}: {
  file: FileRow;
  folder: string | undefined;
  checked: boolean;
  onToggle: () => void;
  /// Present only for a zip, which can be browsed in place.
  onOpen?: () => void;
  onDeleted: () => void;
}) {
  const Icon = iconFor(file.mime_type, file.name);
  const { deleting, remove } = useDelete(file, folder, onDeleted);

  return (
    <div className="group flex items-center gap-3 border-b border-border-soft px-4 py-2.5 transition-colors last:border-0 hover:bg-surface">
      <input
        type="checkbox"
        checked={checked}
        onChange={onToggle}
        aria-label={`Select ${file.name}`}
        className="h-3.5 w-3.5 shrink-0 accent-[var(--color-accent)]"
      />
      <Icon className="h-4 w-4 shrink-0 text-ink-faint" />
      <div className="min-w-0 flex-1">
        {onOpen ? (
          <button
            onClick={onOpen}
            title="Look inside this archive"
            className="block max-w-full truncate text-left text-sm text-ink hover:text-accent hover:underline"
          >
            {file.name}
          </button>
        ) : (
          <p className="truncate text-sm text-ink" title={file.name}>
            {file.name}
          </p>
        )}
      </div>
      <span className="shrink-0 text-xs text-ink-faint">{formatSize(file.size)}</span>
      <span className="hidden shrink-0 text-xs text-ink-faint sm:inline">
        {new Date(file.date * 1000).toLocaleDateString()}
      </span>
      <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
        {onOpen && (
          <button
            onClick={onOpen}
            title="Look inside"
            aria-label={`Look inside ${file.name}`}
            className="rounded-[var(--radius-control)] p-1.5 text-ink-faint hover:bg-canvas hover:text-ink"
          >
            <FolderOpen className="h-3.5 w-3.5" />
          </button>
        )}
        <a
          href={api.files.downloadUrl(file.id, folder)}
          title="Download"
          aria-label={`Download ${file.name}`}
          className="rounded-[var(--radius-control)] p-1.5 text-ink-faint hover:bg-canvas hover:text-ink"
        >
          <Upload className="h-3.5 w-3.5 rotate-180" />
        </a>
        <button
          onClick={() => void remove()}
          disabled={deleting}
          title="Delete permanently"
          aria-label={`Delete ${file.name}`}
          className="rounded-[var(--radius-control)] p-1.5 text-ink-faint hover:bg-[var(--color-danger-soft)] hover:text-[var(--color-danger)] disabled:opacity-40"
        >
          <Trash2 className="h-3.5 w-3.5" />
        </button>
      </div>
    </div>
  );
}

function GridCard({
  file,
  folder,
  onOpen,
  onDeleted,
}: {
  file: FileRow;
  folder: string | undefined;
  onOpen?: () => void;
  onDeleted: () => void;
}) {
  const Icon = iconFor(file.mime_type, file.name);
  const { deleting, remove } = useDelete(file, folder, onDeleted);

  return (
    <div className="group relative flex flex-col items-center rounded-[var(--radius-card)] border border-border bg-raised p-4 transition-colors hover:border-accent/40">
      <button
        onClick={() => void remove()}
        disabled={deleting}
        title="Delete permanently"
        aria-label={`Delete ${file.name}`}
        className="absolute right-1.5 top-1.5 rounded-[var(--radius-control)] p-1.5 text-ink-faint
                   opacity-0 transition-all hover:bg-[var(--color-danger-soft)]
                   hover:text-[var(--color-danger)] group-hover:opacity-100"
      >
        <Trash2 className="h-3.5 w-3.5" />
      </button>

      <Icon className="mb-3 mt-2 h-8 w-8 text-ink-faint" />
      <p className="w-full truncate text-center text-xs text-ink" title={file.name}>
        {file.name}
      </p>
      <p className="mt-0.5 text-[11px] text-ink-faint">{formatSize(file.size)}</p>
      <div className="mt-3 flex w-full gap-1">
        {onOpen && (
          <button
            onClick={onOpen}
            className="flex-1 rounded-[var(--radius-control)] border border-border py-1
                       text-center text-[11px] text-ink-muted transition-colors
                       hover:bg-surface hover:text-ink"
          >
            Open
          </button>
        )}
        <a
          href={api.files.downloadUrl(file.id, folder)}
          className="flex-1 rounded-[var(--radius-control)] border border-border py-1
                     text-center text-[11px] text-ink-muted transition-colors
                     hover:bg-surface hover:text-ink"
        >
          Download
        </a>
      </div>
    </div>
  );
}

/// Only zip: the viewer reads the zip format specifically, and offering to
/// open a .rar it cannot read would be a worse answer than not offering.
function isArchive(file: FileRow) {
  return (
    file.name.toLowerCase().endsWith(".zip") ||
    file.mime_type === "application/zip"
  );
}

function iconFor(mime: string | null, name: string) {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  const type = mime ?? "";
  if (type.startsWith("image/")) return FileImage;
  if (type.startsWith("video/")) return FileVideo;
  if (type.startsWith("audio/")) return FileAudio;
  if (type === "application/pdf" || ext === "pdf") return FileText;
  if (["zip", "rar", "7z", "tar", "gz"].includes(ext)) return FileArchive;
  return FileIcon;
}
