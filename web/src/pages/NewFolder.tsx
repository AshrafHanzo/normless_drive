import { useState } from "react";
import { FolderPlus, X } from "lucide-react";
import { api, ApiError } from "../api";
import { Alert, Button, Card, Field } from "../ui";

/// Replaces `window.prompt`.
///
/// The native prompt is styled by the browser, ignores the app's theme,
/// cannot show an error without a second dialog, and on some platforms
/// looks like a phishing warning. This is the same interaction in the
/// app's own clothes.
export function NewFolder({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: (folderId: string) => void;
}) {
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function create(event: React.FormEvent) {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      setError("Give the folder a name.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const made = await api.folders.create(trimmed);
      onCreated(made.id);
    } catch (caught) {
      setError(
        caught instanceof ApiError
          ? caught.message
          : "Could not create the folder.",
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
        className="my-24 w-full max-w-sm"
        onClick={(event) => event.stopPropagation()}
      >
        <Card className="p-5">
          <div className="mb-4 flex items-start justify-between gap-4">
            <div className="flex gap-3">
              <div className="mt-0.5 flex h-8 w-8 shrink-0 items-center justify-center rounded-[var(--radius-control)] bg-accent-soft">
                <FolderPlus className="h-4 w-4 text-accent" />
              </div>
              <div>
                <h2 className="text-sm font-semibold text-ink">New folder</h2>
                <p className="mt-0.5 text-xs text-ink-muted">
                  Creates a private Telegram channel to hold its files.
                </p>
              </div>
            </div>
            <button
              type="button"
              onClick={onClose}
              aria-label="Close"
              className="text-ink-faint transition-colors hover:text-ink"
            >
              <X className="h-4 w-4" />
            </button>
          </div>

          <form onSubmit={create} className="space-y-3">
            {error && <Alert tone="error">{error}</Alert>}
            <Field
              label="Name"
              autoFocus
              required
              maxLength={100}
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Invoices"
            />
            <div className="flex gap-2">
              <Button
                type="button"
                variant="ghost"
                onClick={onClose}
                className="flex-1"
              >
                Cancel
              </Button>
              <Button type="submit" busy={busy} className="flex-1">
                Create
              </Button>
            </div>
          </form>
        </Card>
      </div>
    </div>
  );
}
