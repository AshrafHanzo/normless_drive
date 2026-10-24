import { useCallback, useEffect, useMemo, useState } from "react";
import {
  ChevronRight,
  CornerLeftUp,
  Download,
  Eye,
  FileArchive,
  Folder as FolderIcon,
  Loader2,
  Search,
  X,
} from "lucide-react";
import {
  extract,
  readArchive,
  RemoteFile,
  ZipError,
  type Archive,
  type ZipEntry,
} from "../zipArchive";
import { childrenOf, formatSize, parentOf, trail } from "../paths";
import { Alert, Spinner } from "../ui";

/// Opening a zip without downloading it, the way Explorer opens one in
/// place.
///
/// A zip keeps its index in the last few kilobytes, so listing a 2 GB
/// archive costs two small range requests. Opening one file inside it costs
/// that file and nothing else — which is the whole point: a folder uploaded
/// as an archive is not a sealed box.
///
/// Nothing here goes through the server beyond those ranged reads; the
/// decompression happens in the tab.
export function ZipViewer({
  url,
  name,
  size,
  onClose,
}: {
  url: string;
  name: string;
  /// The archive's own size, already known from the listing — so no request
  /// is needed just to measure it.
  size: number;
  onClose: () => void;
}) {
  const file = useMemo(() => new RemoteFile(url, size), [url, size]);
  const [archive, setArchive] = useState<Archive | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [cwd, setCwd] = useState("");
  const [query, setQuery] = useState("");
  const [working, setWorking] = useState<string | null>(null);
  const [preview, setPreview] = useState<Preview | null>(null);

  useEffect(() => {
    let live = true;
    readArchive(file)
      .then((result) => live && setArchive(result))
      .catch((caught) => {
        if (!live) return;
        setError(
          caught instanceof ZipError
            ? caught.message
            : "Could not read this archive.",
        );
      });
    return () => {
      live = false;
    };
  }, [file]);

  const closePreview = useCallback(() => {
    setPreview((current) => {
      if (current?.objectUrl) URL.revokeObjectURL(current.objectUrl);
      return null;
    });
  }, []);

  // Revoking on unmount as well, or a preview left open when the viewer
  // closes keeps its blob alive for the life of the tab.
  useEffect(() => closePreview, [closePreview]);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.stopPropagation();
      if (preview) closePreview();
      else onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [preview, closePreview, onClose]);

  const files = useMemo(
    () => (archive?.entries ?? []).filter((entry) => !entry.directory),
    [archive],
  );
  const byPath = useMemo(
    () => new Map(files.map((entry) => [entry.path, entry])),
    [files],
  );

  const needle = query.trim().toLowerCase();
  const rows = useMemo(
    () =>
      needle
        ? files
            .filter((entry) => entry.path.toLowerCase().includes(needle))
            .slice(0, 500)
            .map((entry) => ({
              path: entry.path,
              name: entry.path,
              directory: false,
              size: entry.size,
              count: 1,
            }))
        : childrenOf(
            files.map((entry) => ({ path: entry.path, size: entry.size })),
            cwd,
          ),
    [files, needle, cwd],
  );

  async function read(entry: ZipEntry): Promise<Uint8Array | null> {
    setWorking(entry.path);
    setError(null);
    try {
      return await extract(file, entry);
    } catch (caught) {
      setError(
        caught instanceof ZipError
          ? caught.message
          : `Could not extract ${entry.name}.`,
      );
      return null;
    } finally {
      setWorking(null);
    }
  }

  async function save(entry: ZipEntry) {
    const bytes = await read(entry);
    if (!bytes) return;
    const blob = new Blob([bytes as BlobPart], {
      type: mimeFor(entry.name) ?? "application/octet-stream",
    });
    const href = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = href;
    anchor.download = entry.name;
    anchor.click();
    // Revoked on a later tick: revoking immediately cancels the download in
    // some browsers before it has started.
    window.setTimeout(() => URL.revokeObjectURL(href), 60_000);
  }

  async function show(entry: ZipEntry) {
    const kind = previewKind(entry.name);
    if (!kind) return;
    const bytes = await read(entry);
    if (!bytes) return;

    closePreview();
    if (kind === "text") {
      const slice = bytes.subarray(0, TEXT_LIMIT);
      setPreview({
        entry,
        kind,
        text: new TextDecoder().decode(slice),
        truncated: bytes.length > TEXT_LIMIT,
      });
      return;
    }
    const blob = new Blob([bytes as BlobPart], {
      type: mimeFor(entry.name) ?? "application/octet-stream",
    });
    setPreview({ entry, kind, objectUrl: URL.createObjectURL(blob) });
  }

  const packed = archive
    ? Math.max(0, Math.round((1 - archive.compressedSize / (archive.size || 1)) * 100))
    : 0;

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/60 p-4 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="my-6 flex w-full max-w-3xl flex-col overflow-hidden rounded-[var(--radius-card)] border border-border bg-raised"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="flex items-start gap-3 border-b border-border px-5 py-4">
          <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-[var(--radius-control)] bg-accent-soft">
            <FileArchive className="h-4 w-4 text-accent" />
          </div>
          <div className="min-w-0 flex-1">
            <h2 className="truncate text-sm font-semibold text-ink" title={name}>
              {name}
            </h2>
            <p className="mt-0.5 text-xs text-ink-muted">
              {archive
                ? `${files.length.toLocaleString()} file${files.length === 1 ? "" : "s"} · ${formatSize(
                    archive.size,
                  )} unpacked · ${formatSize(size)} stored${
                    packed > 0 ? ` · ${packed}% smaller` : ""
                  }`
                : "Reading the archive index…"}
            </p>
          </div>
          <a
            href={url}
            className="shrink-0 rounded-[var(--radius-control)] border border-border px-2.5 py-1.5 text-xs text-ink-muted transition-colors hover:bg-surface hover:text-ink"
          >
            Save the whole archive
          </a>
          <button
            onClick={onClose}
            aria-label="Close"
            className="shrink-0 pt-1.5 text-ink-faint transition-colors hover:text-ink"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        {error && (
          <div className="px-5 pt-3">
            <Alert tone="error">{error}</Alert>
          </div>
        )}

        {!archive && !error ? (
          <Spinner />
        ) : archive ? (
          <>
            <div className="flex items-center gap-3 border-b border-border-soft px-5 py-2.5">
              <div className="relative min-w-0 flex-1">
                <Search className="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-ink-faint" />
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="Search inside the archive…"
                  className="w-full rounded-full border border-border bg-canvas py-1.5 pl-9 pr-3 text-sm text-ink outline-none transition-colors placeholder:text-ink-faint focus:border-accent"
                />
              </div>
            </div>

            {!needle && (
              <div className="flex flex-wrap items-center gap-1 border-b border-border-soft px-5 py-2 text-xs">
                <button
                  onClick={() => setCwd("")}
                  className={`rounded px-1.5 py-0.5 transition-colors hover:bg-surface ${
                    cwd ? "text-ink-muted hover:text-ink" : "font-medium text-ink"
                  }`}
                >
                  {name}
                </button>
                {trail(cwd).map((crumb, index, all) => (
                  <span key={crumb.path} className="flex items-center gap-1">
                    <ChevronRight className="h-3 w-3 text-ink-faint" />
                    <button
                      onClick={() => setCwd(crumb.path)}
                      className={`rounded px-1.5 py-0.5 transition-colors hover:bg-surface ${
                        index === all.length - 1
                          ? "font-medium text-ink"
                          : "text-ink-muted hover:text-ink"
                      }`}
                    >
                      {crumb.name}
                    </button>
                  </span>
                ))}
              </div>
            )}

            <div className="max-h-[55vh] min-h-[12rem] overflow-y-auto">
              {!needle && cwd && (
                <button
                  onClick={() => setCwd(parentOf(cwd))}
                  className="flex w-full items-center gap-3 border-b border-border-soft px-5 py-2 text-left transition-colors hover:bg-surface"
                >
                  <CornerLeftUp className="h-4 w-4 shrink-0 text-ink-faint" />
                  <span className="text-sm text-ink-muted">Up one level</span>
                </button>
              )}

              {rows.length === 0 ? (
                <p className="px-5 py-12 text-center text-sm text-ink-faint">
                  {needle ? "Nothing in here matches." : "This folder is empty."}
                </p>
              ) : (
                rows.map((row) => {
                  if (row.directory) {
                    return (
                      <button
                        key={row.path}
                        onClick={() => setCwd(row.path)}
                        className="flex w-full items-center gap-3 border-b border-border-soft px-5 py-2.5 text-left transition-colors last:border-0 hover:bg-surface"
                      >
                        <FolderIcon className="h-4 w-4 shrink-0 text-accent" />
                        <span className="min-w-0 flex-1 truncate text-sm text-ink">
                          {row.name}
                        </span>
                        <span className="shrink-0 text-xs text-ink-faint">
                          {row.count} item{row.count === 1 ? "" : "s"}
                        </span>
                        <span className="w-20 shrink-0 text-right text-xs text-ink-faint">
                          {formatSize(row.size)}
                        </span>
                      </button>
                    );
                  }

                  const entry = byPath.get(row.path);
                  if (!entry) return null;
                  const busy = working === entry.path;
                  const viewable = previewKind(entry.name) !== null;

                  return (
                    <div
                      key={row.path}
                      className="group flex items-center gap-3 border-b border-border-soft px-5 py-2.5 transition-colors last:border-0 hover:bg-surface"
                    >
                      <FileArchive className="h-4 w-4 shrink-0 text-ink-faint" />
                      <div className="min-w-0 flex-1">
                        <p className="truncate text-sm text-ink" title={row.name}>
                          {row.name}
                        </p>
                        {entry.modified && (
                          <p className="text-[11px] text-ink-faint">
                            {entry.modified.toLocaleString()}
                          </p>
                        )}
                      </div>
                      <span className="w-20 shrink-0 text-right text-xs text-ink-faint">
                        {formatSize(entry.size)}
                      </span>
                      <div className="flex w-16 shrink-0 items-center justify-end gap-0.5">
                        {busy ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin text-ink-faint" />
                        ) : (
                          <>
                            {viewable && (
                              <button
                                onClick={() => void show(entry)}
                                title="Preview"
                                aria-label={`Preview ${entry.name}`}
                                className="rounded-[var(--radius-control)] p-1.5 text-ink-faint opacity-0 transition-opacity hover:bg-canvas hover:text-ink group-hover:opacity-100 focus:opacity-100"
                              >
                                <Eye className="h-3.5 w-3.5" />
                              </button>
                            )}
                            <button
                              onClick={() => void save(entry)}
                              title="Save this file"
                              aria-label={`Save ${entry.name}`}
                              className="rounded-[var(--radius-control)] p-1.5 text-ink-faint opacity-0 transition-opacity hover:bg-canvas hover:text-ink group-hover:opacity-100 focus:opacity-100"
                            >
                              <Download className="h-3.5 w-3.5" />
                            </button>
                          </>
                        )}
                      </div>
                    </div>
                  );
                })
              )}
            </div>

            <footer className="border-t border-border px-5 py-2.5 text-[11px] leading-relaxed text-ink-faint">
              Only the parts you open are downloaded — the list above came
              from the last few kilobytes of the archive.
            </footer>
          </>
        ) : null}
      </div>

      {preview && <PreviewPane preview={preview} onClose={closePreview} />}
    </div>
  );
}

interface Preview {
  entry: ZipEntry;
  kind: "image" | "pdf" | "video" | "audio" | "text";
  objectUrl?: string;
  text?: string;
  truncated?: boolean;
}

/// Shown above the listing rather than beside it: the point of a preview is
/// to look at the thing, and a side panel in a dialog gives it a third of
/// the width.
function PreviewPane({
  preview,
  onClose,
}: {
  preview: Preview;
  onClose: () => void;
}) {
  const { entry, kind, objectUrl, text, truncated } = preview;

  return (
    <div
      className="fixed inset-0 z-[60] flex flex-col bg-black/80 p-4 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="mx-auto flex h-full w-full max-w-5xl flex-col overflow-hidden rounded-[var(--radius-card)] border border-border bg-raised"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="flex items-center gap-3 border-b border-border px-4 py-2.5">
          <p className="min-w-0 flex-1 truncate text-sm text-ink" title={entry.path}>
            {entry.name}
          </p>
          <span className="shrink-0 text-xs text-ink-faint">
            {formatSize(entry.size)}
          </span>
          <button
            onClick={onClose}
            aria-label="Close preview"
            className="shrink-0 text-ink-faint transition-colors hover:text-ink"
          >
            <X className="h-4 w-4" />
          </button>
        </header>

        <div className="flex min-h-0 flex-1 items-center justify-center overflow-auto bg-canvas">
          {kind === "image" && objectUrl && (
            <img
              src={objectUrl}
              alt={entry.name}
              className="max-h-full max-w-full object-contain"
            />
          )}
          {kind === "pdf" && objectUrl && (
            <iframe src={objectUrl} title={entry.name} className="h-full w-full" />
          )}
          {kind === "video" && objectUrl && (
            <video src={objectUrl} controls className="max-h-full max-w-full" />
          )}
          {kind === "audio" && objectUrl && (
            <audio src={objectUrl} controls className="w-full max-w-md" />
          )}
          {kind === "text" && (
            <pre className="h-full w-full overflow-auto p-4 text-left font-mono text-xs leading-relaxed text-ink">
              {text}
              {truncated && (
                <span className="mt-4 block text-ink-faint">
                  — truncated. Save the file to read the rest.
                </span>
              )}
            </pre>
          )}
        </div>
      </div>
    </div>
  );
}

/// Shown inline rather than streamed, so the whole thing is decompressed
/// first — hence a ceiling on what is worth previewing.
const TEXT_LIMIT = 512 * 1024;

const IMAGES = ["png", "jpg", "jpeg", "gif", "webp", "avif", "bmp", "ico", "svg"];
const VIDEOS = ["mp4", "webm", "ogv", "mov"];
const AUDIO = ["mp3", "wav", "ogg", "m4a", "flac"];
const TEXTS = [
  "txt", "md", "json", "csv", "tsv", "log", "xml", "yml", "yaml", "ini", "cfg",
  "conf", "toml", "js", "ts", "tsx", "jsx", "css", "scss", "html", "htm", "rs",
  "py", "go", "java", "c", "h", "cpp", "sh", "sql", "env", "gitignore",
];

function extensionOf(name: string) {
  return name.split(".").pop()?.toLowerCase() ?? "";
}

function previewKind(name: string): Preview["kind"] | null {
  const ext = extensionOf(name);
  if (IMAGES.includes(ext)) return "image";
  if (ext === "pdf") return "pdf";
  if (VIDEOS.includes(ext)) return "video";
  if (AUDIO.includes(ext)) return "audio";
  if (TEXTS.includes(ext)) return "text";
  return null;
}

function mimeFor(name: string): string | null {
  const ext = extensionOf(name);
  const known: Record<string, string> = {
    png: "image/png",
    jpg: "image/jpeg",
    jpeg: "image/jpeg",
    gif: "image/gif",
    webp: "image/webp",
    avif: "image/avif",
    bmp: "image/bmp",
    ico: "image/x-icon",
    svg: "image/svg+xml",
    pdf: "application/pdf",
    mp4: "video/mp4",
    webm: "video/webm",
    mov: "video/quicktime",
    mp3: "audio/mpeg",
    wav: "audio/wav",
    ogg: "audio/ogg",
    m4a: "audio/mp4",
    flac: "audio/flac",
    json: "application/json",
    csv: "text/csv",
    html: "text/html",
    htm: "text/html",
    txt: "text/plain",
    md: "text/markdown",
  };
  return known[ext] ?? null;
}
