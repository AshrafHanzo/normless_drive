import { useMemo, useState } from "react";
import {
  ChevronRight,
  CornerLeftUp,
  FileArchive,
  Folder as FolderIcon,
  FolderUp,
  Files,
  Upload,
  X,
} from "lucide-react";
import type { Picked } from "../folderPick";
import {
  loadZipPreference,
  saveZipPreference,
  totalBytes,
  ZIP_LIMIT_BYTES,
  type UploadMode,
} from "../folderUpload";
import { childrenOf, formatSize, parentOf, trail } from "../paths";
import { Button, Card } from "../ui";

/// What was picked, and what is about to happen to it.
///
/// The browser's own folder prompt says only how many files it found. This
/// says which ones, how big they are, and which of the two upload shapes
/// they will arrive in — before anything leaves the machine.
export function FolderReview({
  picked,
  busy = false,
  onCancel,
  onConfirm,
}: {
  picked: Picked;
  busy?: boolean;
  onCancel: () => void;
  onConfirm: (mode: UploadMode) => void;
}) {
  const [cwd, setCwd] = useState("");
  const bytes = useMemo(() => totalBytes(picked.files), [picked]);
  const tooBigToZip = bytes > ZIP_LIMIT_BYTES;

  const [mode, setMode] = useState<UploadMode>(
    tooBigToZip ? "individual" : loadZipPreference() ? "zip" : "individual",
  );

  const items = useMemo(
    () => picked.files.map((item) => ({ path: item.path, size: item.file.size })),
    [picked],
  );
  const rows = useMemo(() => childrenOf(items, cwd), [items, cwd]);
  const crumbs = trail(cwd);

  function choose(next: UploadMode) {
    if (next === "zip" && tooBigToZip) return;
    setMode(next);
    saveZipPreference(next === "zip");
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/50 p-4 backdrop-blur-sm"
      onClick={busy ? undefined : onCancel}
    >
      <div
        className="my-8 w-full max-w-2xl"
        onClick={(event) => event.stopPropagation()}
      >
        <Card className="overflow-hidden">
          <header className="flex items-start gap-3 border-b border-border px-5 py-4">
            <div className="flex h-9 w-9 shrink-0 items-center justify-center rounded-[var(--radius-control)] bg-accent-soft">
              <FolderUp className="h-4 w-4 text-accent" />
            </div>
            <div className="min-w-0 flex-1">
              <h2 className="truncate text-sm font-semibold text-ink" title={picked.name}>
                {picked.name}
              </h2>
              <p className="mt-0.5 text-xs text-ink-muted">
                {picked.files.length.toLocaleString()} file
                {picked.files.length === 1 ? "" : "s"} · {formatSize(bytes)} · ready
                to upload
              </p>
            </div>
            <button
              onClick={onCancel}
              disabled={busy}
              aria-label="Cancel"
              className="text-ink-faint transition-colors hover:text-ink disabled:opacity-40"
            >
              <X className="h-4 w-4" />
            </button>
          </header>

          <section className="grid gap-2 px-5 py-4 sm:grid-cols-2">
            <ModeCard
              icon={FileArchive}
              title="As one .zip"
              detail="Structure kept exactly. You can still open the archive here and pull one file out of it without downloading the rest."
              selected={mode === "zip"}
              disabled={tooBigToZip}
              disabledReason={`Too large to zip in the browser — the archive is built in memory, and ${formatSize(bytes)} will not fit.`}
              onSelect={() => choose("zip")}
            />
            <ModeCard
              icon={Files}
              title="As separate files"
              detail="Each file downloadable on its own, with its path kept in the name. No size limit."
              selected={mode === "individual"}
              onSelect={() => choose("individual")}
            />
          </section>

          <div className="border-t border-border-soft">
            <div className="flex items-center gap-1 px-5 py-2 text-xs">
              <button
                onClick={() => setCwd("")}
                className={`rounded px-1.5 py-0.5 transition-colors hover:bg-surface ${
                  cwd ? "text-ink-muted hover:text-ink" : "font-medium text-ink"
                }`}
              >
                {picked.name}
              </button>
              {crumbs.map((crumb, index) => (
                <span key={crumb.path} className="flex items-center gap-1">
                  <ChevronRight className="h-3 w-3 text-ink-faint" />
                  <button
                    onClick={() => setCwd(crumb.path)}
                    className={`rounded px-1.5 py-0.5 transition-colors hover:bg-surface ${
                      index === crumbs.length - 1
                        ? "font-medium text-ink"
                        : "text-ink-muted hover:text-ink"
                    }`}
                  >
                    {crumb.name}
                  </button>
                </span>
              ))}
            </div>

            <div className="max-h-64 overflow-y-auto border-t border-border-soft">
              {cwd && (
                <button
                  onClick={() => setCwd(parentOf(cwd))}
                  className="flex w-full items-center gap-3 border-b border-border-soft px-5 py-2 text-left transition-colors hover:bg-surface"
                >
                  <CornerLeftUp className="h-4 w-4 shrink-0 text-ink-faint" />
                  <span className="text-sm text-ink-muted">Up one level</span>
                </button>
              )}
              {rows.map((row) =>
                row.directory ? (
                  <button
                    key={row.path}
                    onClick={() => setCwd(row.path)}
                    className="flex w-full items-center gap-3 border-b border-border-soft px-5 py-2 text-left transition-colors last:border-0 hover:bg-surface"
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
                ) : (
                  <div
                    key={row.path}
                    className="flex items-center gap-3 border-b border-border-soft px-5 py-2 last:border-0"
                  >
                    <span className="h-4 w-4 shrink-0" />
                    <span
                      className="min-w-0 flex-1 truncate text-sm text-ink-muted"
                      title={row.name}
                    >
                      {row.name}
                    </span>
                    <span className="w-20 shrink-0 text-right text-xs text-ink-faint">
                      {formatSize(row.size)}
                    </span>
                  </div>
                ),
              )}
            </div>
          </div>

          <footer className="flex items-center justify-end gap-2 border-t border-border px-5 py-3">
            <Button variant="ghost" onClick={onCancel} disabled={busy}>
              Cancel
            </Button>
            <Button busy={busy} onClick={() => onConfirm(mode)}>
              <Upload className="h-3.5 w-3.5" />
              {mode === "zip"
                ? `Upload as ${picked.name}.zip`
                : `Upload ${picked.files.length.toLocaleString()} files`}
            </Button>
          </footer>
        </Card>
      </div>
    </div>
  );
}

function ModeCard({
  icon: Icon,
  title,
  detail,
  selected,
  disabled = false,
  disabledReason,
  onSelect,
}: {
  icon: typeof FileArchive;
  title: string;
  detail: string;
  selected: boolean;
  disabled?: boolean;
  disabledReason?: string;
  onSelect: () => void;
}) {
  return (
    <button
      onClick={onSelect}
      disabled={disabled}
      className={`rounded-[var(--radius-control)] border p-3 text-left transition-colors ${
        disabled
          ? "cursor-not-allowed border-border opacity-60"
          : selected
            ? "border-accent bg-accent-soft"
            : "border-border hover:border-accent/40 hover:bg-surface"
      }`}
    >
      <span className="flex items-center gap-2">
        <Icon
          className={`h-4 w-4 ${selected && !disabled ? "text-accent" : "text-ink-faint"}`}
        />
        <span
          className={`text-sm font-medium ${
            selected && !disabled ? "text-accent" : "text-ink"
          }`}
        >
          {title}
        </span>
      </span>
      <span className="mt-1 block text-xs leading-relaxed text-ink-faint">
        {disabled ? disabledReason : detail}
      </span>
    </button>
  );
}
