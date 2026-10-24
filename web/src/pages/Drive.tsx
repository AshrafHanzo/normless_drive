import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  FileArchive,
  FileAudio,
  FileImage,
  FileText,
  FileVideo,
  File as FileIcon,
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
import { ShareDialog } from "./ShareDialog";

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
  const [prefs, setPrefs] = useState(false);
  const [remote, setRemote] = useState(false);
  const [dragging, setDragging] = useState(false);
  const [newFolder, setNewFolder] = useState(false);
  const [syncing, setSyncing] = useState(false);
  const [newGroup, setNewGroup] = useState(false);
  const [queue, setQueue] = useState<QueueItem[]>([]);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [groups, setGroups] = useState<Group[]>([]);
  const picker = useRef<HTMLInputElement>(null);
  const searchBox = useRef<HTMLInputElement>(null);

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

  /// Uploads a batch, one at a time.
  ///
  /// Sequential on purpose: every byte of every file crosses this server
  /// on its way to Telegram, so running them in parallel does not make
  /// the total any faster — it just makes each one slower and the
  /// progress harder to read.
  async function send(files: File[]) {
    if (files.length === 0) return;

    setUploading(true);
    setError(null);
    setQueue(
      files.map((file, index) => ({
        id: `${index}-${file.name}-${file.size}`,
        name: file.name,
        size: file.size,
        state: "waiting" as const,
      })),
    );

    for (const [index, file] of files.entries()) {
      setQueue((current) =>
        current.map((item, i) =>
          i === index ? { ...item, state: "uploading" } : item,
        ),
      );
      try {
        await api.files.upload(file, folder);
        setQueue((current) =>
          current.map((item, i) =>
            i === index ? { ...item, state: "done" } : item,
          ),
        );
      } catch (caught) {
        const message =
          caught instanceof ApiError ? caught.message : "Upload failed.";
        setQueue((current) =>
          current.map((item, i) =>
            i === index ? { ...item, state: "failed", error: message } : item,
          ),
        );
      }
    }

    setUploading(false);
    if (picker.current) picker.current.value = "";
    await load(folder);
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
        if (dropped.length) void send(dropped);
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

            <Button busy={uploading} onClick={() => picker.current?.click()}>
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
                if (chosen.length) void send(chosen);
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
  onDeleted,
}: {
  file: FileRow;
  folder: string | undefined;
  checked: boolean;
  onToggle: () => void;
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
        <p className="truncate text-sm text-ink" title={file.name}>
          {file.name}
        </p>
      </div>
      <span className="shrink-0 text-xs text-ink-faint">{formatSize(file.size)}</span>
      <span className="hidden shrink-0 text-xs text-ink-faint sm:inline">
        {new Date(file.date * 1000).toLocaleDateString()}
      </span>
      <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100 focus-within:opacity-100">
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
  onDeleted,
}: {
  file: FileRow;
  folder: string | undefined;
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
      <a
        href={api.files.downloadUrl(file.id, folder)}
        className="mt-3 w-full rounded-[var(--radius-control)] border border-border py-1
                   text-center text-[11px] text-ink-muted transition-colors
                   hover:bg-surface hover:text-ink"
      >
        Download
      </a>
    </div>
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

function formatSize(bytes: number) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}
