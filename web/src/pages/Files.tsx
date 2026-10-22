import { useCallback, useEffect, useRef, useState } from "react";
import {
  Download,
  FileArchive,
  FileAudio,
  FileImage,
  FileText,
  FileVideo,
  FolderPlus,
  Share2,
  File as FileIcon,
  RotateCw,
  Unplug,
  Upload,
} from "lucide-react";
import { api, ApiError, type FileRow, type Folder } from "../api";
import { Alert, Button, Card, Spinner } from "../ui";
import { ShareDialog } from "./ShareDialog";

export function Files({
  phoneHint,
  onDisconnected,
}: {
  phoneHint: string | null;
  onDisconnected: () => void;
}) {
  const [files, setFiles] = useState<FileRow[] | null>(null);
  const [folders, setFolders] = useState<Folder[]>([]);
  // undefined means Saved Messages, which has no folder row of its own.
  const [folder, setFolder] = useState<string | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [uploading, setUploading] = useState(false);
  const [progress, setProgress] = useState<string | null>(null);
  const [creating, setCreating] = useState(false);
  const picker = useRef<HTMLInputElement>(null);
  const [sharing, setSharing] = useState(false);

  const load = useCallback(async (which?: string) => {
    setError(null);
    setFiles(null);
    try {
      setFiles((await api.files.list(which)).files);
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "Could not load files.",
      );
      setFiles([]);
    }
  }, []);

  const loadFolders = useCallback(async () => {
    try {
      setFolders((await api.folders.list()).folders);
    } catch {
      // A folder list that fails should not stop the files loading —
      // Saved Messages still works without it.
    }
  }, []);

  useEffect(() => {
    void load(folder);
  }, [load, folder]);

  useEffect(() => {
    void loadFolders();
  }, [loadFolders]);

  async function send(file: File) {
    setUploading(true);
    setError(null);
    setProgress(`Uploading ${file.name}…`);
    try {
      await api.files.upload(file, folder);
      await load(folder);
      setProgress(null);
    } catch (caught) {
      setProgress(null);
      setError(caught instanceof ApiError ? caught.message : "Upload failed.");
    } finally {
      setUploading(false);
      // Reset so picking the same file twice still fires a change event.
      if (picker.current) picker.current.value = "";
    }
  }

  async function addFolder() {
    const name = window.prompt("Folder name");
    if (!name?.trim()) return;
    setCreating(true);
    setError(null);
    try {
      const made = await api.folders.create(name.trim());
      await loadFolders();
      setFolder(made.id);
    } catch (caught) {
      setError(
        caught instanceof ApiError
          ? caught.message
          : "Could not create the folder.",
      );
    } finally {
      setCreating(false);
    }
  }

  const activeName =
    folder === undefined
      ? "Saved Messages"
      : (folders.find((f) => f.id === folder)?.name ?? "Folder");

  return (
    <div
      className="mx-auto max-w-5xl px-4 py-8"
      onDragOver={(e) => e.preventDefault()}
      onDrop={(e) => {
        e.preventDefault();
        const dropped = e.dataTransfer.files?.[0];
        if (dropped) void send(dropped);
      }}
    >
      <header className="mb-5 flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold text-ink">{activeName}</h1>
          <p className="text-sm text-ink-muted">
            {files === null
              ? "Loading…"
              : `${files.length} file${files.length === 1 ? "" : "s"}`}
            {phoneHint && ` · account ${phoneHint}`}
          </p>
        </div>

        <div className="flex gap-2">
          <Button variant="ghost" onClick={() => setSharing(true)}>
            <Share2 className="h-3.5 w-3.5" />
            Share
          </Button>
          <Button variant="ghost" busy={creating} onClick={() => void addFolder()}>
            <FolderPlus className="h-3.5 w-3.5" />
            New folder
          </Button>
          <Button variant="ghost" onClick={() => void load(folder)}>
            <RotateCw className="h-3.5 w-3.5" />
            Refresh
          </Button>
          <Button busy={uploading} onClick={() => picker.current?.click()}>
            <Upload className="h-3.5 w-3.5" />
            Upload
          </Button>
          <input
            ref={picker}
            type="file"
            hidden
            onChange={(e) => {
              const chosen = e.target.files?.[0];
              if (chosen) void send(chosen);
            }}
          />
        </div>
      </header>

      {/* Folder tabs. Saved Messages is always first and always present —
          it is the one location that needs no setup. */}
      <div className="mb-4 flex flex-wrap gap-1.5">
        <FolderTab
          label="Saved Messages"
          active={folder === undefined}
          onClick={() => setFolder(undefined)}
        />
        {folders.map((item) => (
          <FolderTab
            key={item.id}
            label={item.name}
            active={folder === item.id}
            onClick={() => setFolder(item.id)}
          />
        ))}
      </div>

      {sharing && (
        <ShareDialog
          folderId={folder}
          folderName={activeName}
          onClose={() => setSharing(false)}
        />
      )}


      {error && (
        <div className="mb-4">
          <Alert tone="error">{error}</Alert>
        </div>
      )}
      {progress && (
        <div className="mb-4">
          <Alert tone="warn">{progress}</Alert>
        </div>
      )}

      {files === null ? (
        <Spinner />
      ) : files.length === 0 ? (
        <Card className="px-6 py-16 text-center">
          <p className="text-sm font-medium text-ink">Nothing here yet</p>
          <p className="mx-auto mt-1 max-w-sm text-xs text-ink-muted">
            Upload a file, or drag one onto this page. It goes straight to your
            own Telegram Saved Messages.
          </p>
        </Card>
      ) : (
        <Card className="divide-y divide-border-soft">
          {files.map((file) => (
            <Row key={file.id} file={file} folder={folder} />
          ))}
        </Card>
      )}

      <div className="mt-6 text-center">
        <Button
          variant="ghost"
          onClick={async () => {
            await api.telegram.disconnect().catch(() => undefined);
            onDisconnected();
          }}
        >
          <Unplug className="h-3.5 w-3.5" />
          Disconnect Telegram
        </Button>
      </div>
    </div>
  );
}

function FolderTab({
  label,
  active,
  onClick,
}: {
  label: string;
  active: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      className={`rounded-full px-3 py-1 text-xs font-medium transition-colors ${
        active
          ? "bg-accent text-white"
          : "border border-border text-ink-muted hover:bg-surface hover:text-ink"
      }`}
    >
      {label}
    </button>
  );
}

function Row({ file, folder }: { file: FileRow; folder: string | undefined }) {
  const Icon = iconFor(file.mime_type, file.name);
  return (
    <div className="flex items-center gap-3 px-4 py-3">
      <Icon className="h-4 w-4 shrink-0 text-ink-faint" />
      <div className="min-w-0 flex-1">
        <p className="truncate text-sm text-ink" title={file.name}>
          {file.name}
        </p>
        <p className="text-xs text-ink-faint">
          {formatSize(file.size)} · {new Date(file.date * 1000).toLocaleDateString()}
        </p>
      </div>
      {/* A plain link, not fetch(): the browser streams it straight to disk
          with a real progress bar, and a 2 GB file never passes through JS
          memory. */}
      <a
        href={api.files.downloadUrl(file.id, folder)}
        className="flex items-center gap-1.5 rounded-[var(--radius-control)] border border-border
                   px-2.5 py-1.5 text-xs text-ink-muted transition-colors
                   hover:bg-surface hover:text-ink"
      >
        <Download className="h-3.5 w-3.5" />
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
