import { CheckCircle2, CircleSlash, Loader2, X, XCircle } from "lucide-react";
import { Card } from "../ui";

export type QueueItem = {
  id: string;
  name: string;
  size: number;
  state: "waiting" | "uploading" | "done" | "failed" | "cancelled";
  error?: string;
};

/// A compact transfer list, like the desktop app's transfer centre.
///
/// Fixed to the corner rather than inline: an upload of twenty files
/// should not push the file list around while you are still reading it,
/// and you need to keep browsing while they run.
export function UploadQueue({
  items,
  onDismiss,
  onStop,
}: {
  items: QueueItem[];
  onDismiss: () => void;
  /// Stops what is running and drops whatever is still queued.
  onStop: () => void;
}) {
  if (items.length === 0) return null;

  const done = items.filter((i) => i.state === "done").length;
  const failed = items.filter((i) => i.state === "failed").length;
  const stopped = items.filter((i) => i.state === "cancelled").length;
  const finished = done + failed + stopped === items.length;

  const summary = finished
    ? [
        `${done} uploaded`,
        failed ? `${failed} failed` : "",
        stopped ? `${stopped} stopped` : "",
      ]
        .filter(Boolean)
        .join(", ")
    : `Uploading ${done + 1} of ${items.length}`;

  return (
    <div className="pointer-events-none fixed bottom-4 right-4 z-40 w-80 max-w-[calc(100vw-2rem)]">
      <Card className="pointer-events-auto overflow-hidden shadow-lg">
        <div className="flex items-center justify-between gap-2 border-b border-border-soft px-3 py-2">
          <span className="text-xs font-medium text-ink">{summary}</span>
          {finished ? (
            <button
              onClick={onDismiss}
              aria-label="Dismiss"
              className="text-ink-faint transition-colors hover:text-ink"
            >
              <X className="h-3.5 w-3.5" />
            </button>
          ) : (
            // Worth its own word rather than an icon: stopping a ten
            // gigabyte upload is not something to do by accident, and
            // not something to have to hunt for either.
            <button
              onClick={onStop}
              className="shrink-0 rounded-[var(--radius-control)] border border-border px-2 py-0.5
                         text-[11px] text-ink-muted transition-colors
                         hover:border-[var(--color-danger)] hover:text-[var(--color-danger)]"
            >
              Stop
            </button>
          )}
        </div>

        <div className="max-h-56 overflow-y-auto">
          {items.map((item) => (
            <div
              key={item.id}
              className="flex items-start gap-2.5 px-3 py-2 text-xs"
            >
              <Icon state={item.state} />
              <span className="min-w-0 flex-1">
                <span className="block truncate text-ink" title={item.name}>
                  {item.name}
                </span>
                {/* Not truncated: an upload that failed should say why
                    here, not hide the reason behind an ellipsis. */}
                {item.error && (
                  <span className="mt-0.5 block text-[11px] leading-snug text-[var(--color-danger)]">
                    {item.error}
                  </span>
                )}
              </span>
              <span className="shrink-0 text-[11px] text-ink-faint">
                {formatSize(item.size)}
              </span>
            </div>
          ))}
        </div>
      </Card>
    </div>
  );
}

function Icon({ state }: { state: QueueItem["state"] }) {
  if (state === "uploading")
    return <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin text-accent" />;
  if (state === "done")
    return (
      <CheckCircle2 className="h-3.5 w-3.5 shrink-0 text-[var(--color-success)]" />
    );
  if (state === "failed")
    return <XCircle className="h-3.5 w-3.5 shrink-0 text-[var(--color-danger)]" />;
  if (state === "cancelled")
    return <CircleSlash className="h-3.5 w-3.5 shrink-0 text-ink-faint" />;
  return <span className="h-3.5 w-3.5 shrink-0 rounded-full border border-border" />;
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
