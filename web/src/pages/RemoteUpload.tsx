import { useState } from "react";
import { Globe, X } from "lucide-react";
import { api, ApiError } from "../api";
import { Alert, Button, Card, Field } from "../ui";

/// Hand the server a URL and it fetches the file straight into Telegram.
///
/// Worth having because the server's connection is usually far better than
/// the one the browser is on — the bytes never travel to this machine.
export function RemoteUpload({
  folder,
  onClose,
  onUploaded,
}: {
  folder: string | undefined;
  onClose: () => void;
  onUploaded: () => void;
}) {
  const [url, setUrl] = useState("");
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function start(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.files.remote(url.trim(), folder, name.trim() || undefined);
      onUploaded();
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "That upload failed.",
      );
      setBusy(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/50 p-4 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="my-16 w-full max-w-md"
        onClick={(event) => event.stopPropagation()}
      >
        <Card className="p-5">
          <div className="mb-4 flex items-start justify-between gap-4">
            <div className="flex gap-3">
              <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--radius-control)] bg-accent-soft">
                <Globe className="h-4 w-4 text-accent" />
              </div>
              <div>
                <h2 className="text-sm font-semibold text-ink">
                  Upload from a URL
                </h2>
                <p className="mt-0.5 text-xs text-ink-muted">
                  The server fetches it directly — nothing downloads to this
                  computer.
                </p>
              </div>
            </div>
            <button
              onClick={onClose}
              aria-label="Close"
              className="text-ink-faint transition-colors hover:text-ink"
            >
              <X className="h-4 w-4" />
            </button>
          </div>

          <form onSubmit={start} className="space-y-3">
            {error && <Alert tone="error">{error}</Alert>}

            <Field
              label="File URL"
              type="url"
              autoFocus
              required
              value={url}
              onChange={(event) => setUrl(event.target.value)}
              placeholder="https://example.com/archive.zip"
              hint="A direct link to the file, not a download page."
            />

            <Field
              label="Save as (optional)"
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Taken from the URL"
            />

            <Button type="submit" busy={busy} className="w-full">
              {busy ? "Fetching…" : "Start upload"}
            </Button>

            {busy && (
              <p className="text-center text-xs text-ink-faint">
                Large files can take a while. Leave this open until it
                finishes.
              </p>
            )}
          </form>
        </Card>
      </div>
    </div>
  );
}
