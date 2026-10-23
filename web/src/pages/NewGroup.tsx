import { useState } from "react";
import { Tag, X } from "lucide-react";
import { api, ApiError } from "../api";
import { Alert, Button, Card, Field } from "../ui";

/// Fixed palette rather than a colour picker.
///
/// These are chosen to stay legible as small dots against both the light
/// and dark surfaces; an arbitrary picker would let someone choose a
/// colour invisible in one theme and then wonder where their group went.
const COLOURS = [
  "#2d7ff9",
  "#1a8a52",
  "#b06a00",
  "#d4493b",
  "#7c4dff",
  "#0097a7",
];

export function NewGroup({
  onClose,
  onCreated,
}: {
  onClose: () => void;
  onCreated: () => void;
}) {
  const [name, setName] = useState("");
  const [colour, setColour] = useState(COLOURS[0]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function create(event: React.FormEvent) {
    event.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      setError("Give the group a name.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api.groups.create(trimmed, colour);
      onCreated();
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "Could not create it.",
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
                <Tag className="h-4 w-4 text-accent" />
              </div>
              <div>
                <h2 className="text-sm font-semibold text-ink">New group</h2>
                <p className="mt-0.5 text-xs leading-relaxed text-ink-muted">
                  A label for organising folders in the sidebar. It holds no
                  files and is not sent to Telegram.
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
              maxLength={60}
              value={name}
              onChange={(event) => setName(event.target.value)}
              placeholder="Work"
            />

            <div>
              <span className="mb-1.5 block text-xs font-medium text-ink-muted">
                Colour
              </span>
              <div className="flex gap-2">
                {COLOURS.map((option) => (
                  <button
                    key={option}
                    type="button"
                    onClick={() => setColour(option)}
                    aria-label={`Use colour ${option}`}
                    className={`h-7 w-7 rounded-full transition-transform ${
                      colour === option
                        ? "scale-110 ring-2 ring-accent ring-offset-2 ring-offset-[var(--color-raised)]"
                        : "hover:scale-105"
                    }`}
                    style={{ backgroundColor: option }}
                  />
                ))}
              </div>
            </div>

            <div className="flex gap-2 pt-1">
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
