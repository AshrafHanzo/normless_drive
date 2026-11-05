import { useState } from "react";
import { Check, FolderPlus, Inbox, Folder as FolderIcon, X } from "lucide-react";
import { api, ApiError, type Folder } from "../api";
import { Alert, Button, Card, Field } from "../ui";

/// Picks where a selection of files should go.
///
/// Telegram has no move, so underneath this is a forward followed by a
/// delete — but the forward happens on Telegram's servers, which is why
/// moving gigabytes takes about as long as moving one file. Said plainly at
/// the bottom rather than hidden, because the copy does get a new date and
/// a "forwarded" marker in the Telegram app, and that surprises people.
export function MoveDialog({
  ids,
  currentFolder,
  folders,
  onClose,
  onMoved,
}: {
  ids: number[];
  /// Where the files are now — it cannot also be the destination.
  currentFolder: string | undefined;
  folders: Folder[];
  onClose: () => void;
  onMoved: (summary: string) => void;
}) {
  const count = ids.length;
  const [target, setTarget] = useState<string | undefined>(undefined);
  const [chosen, setChosen] = useState(false);
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const destinations = [
    ...(currentFolder === undefined
      ? []
      : [{ id: undefined, name: "Saved Messages", saved: true }]),
    ...folders
      .filter((folder) => folder.id !== currentFolder)
      .map((folder) => ({ id: folder.id as string | undefined, name: folder.name, saved: false })),
  ];

  async function go() {
    setBusy(true);
    setError(null);
    try {
      let destination = target;

      // Created first, so a failure here stops before anything has moved.
      if (creating) {
        const name = newName.trim();
        if (!name) {
          setError("Give the new folder a name.");
          setBusy(false);
          return;
        }
        destination = (await api.folders.create(name)).id;
      }

      const result = await api.files.move(ids, currentFolder, destination);
      onMoved(
        result.failed > 0
          ? `Moved ${result.moved}. ${result.failed} could not be moved and are still here.`
          : `Moved ${result.moved} file${result.moved === 1 ? "" : "s"}.`,
      );
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "Could not move those files.",
      );
      setBusy(false);
    }
  }

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/50 p-4 backdrop-blur-sm"
      onClick={busy ? undefined : onClose}
    >
      <div
        className="my-8 w-full max-w-md"
        onClick={(event) => event.stopPropagation()}
      >
        <Card className="p-5">
          <div className="mb-4 flex items-start justify-between gap-4">
            <div>
              <h2 className="text-sm font-semibold text-ink">
                Move {count} file{count === 1 ? "" : "s"}
              </h2>
              <p className="text-xs text-ink-muted">Choose where they should go.</p>
            </div>
            <button
              onClick={onClose}
              disabled={busy}
              aria-label="Close"
              className="text-ink-faint transition-colors hover:text-ink disabled:opacity-40"
            >
              <X className="h-4 w-4" />
            </button>
          </div>

          {error && (
            <div className="mb-3">
              <Alert tone="error">{error}</Alert>
            </div>
          )}

          <div className="max-h-64 space-y-1 overflow-y-auto">
            {destinations.length === 0 && !creating && (
              <p className="py-3 text-xs text-ink-faint">
                There is nowhere else to put them yet — make a folder below.
              </p>
            )}
            {destinations.map((destination) => {
              const selected = !creating && chosen && destination.id === target;
              const Icon = destination.saved ? Inbox : FolderIcon;
              return (
                <button
                  key={destination.id ?? "saved"}
                  onClick={() => {
                    setCreating(false);
                    setChosen(true);
                    setTarget(destination.id);
                  }}
                  className={`flex w-full items-center gap-2.5 rounded-[var(--radius-control)] border px-3 py-2 text-left transition-colors ${
                    selected
                      ? "border-accent bg-accent-soft"
                      : "border-border hover:bg-surface"
                  }`}
                >
                  <Icon
                    className={`h-4 w-4 shrink-0 ${selected ? "text-accent" : "text-ink-faint"}`}
                  />
                  <span
                    className={`min-w-0 flex-1 truncate text-sm ${
                      selected ? "text-accent" : "text-ink"
                    }`}
                  >
                    {destination.name}
                  </span>
                  {selected && <Check className="h-3.5 w-3.5 shrink-0 text-accent" />}
                </button>
              );
            })}

            <button
              onClick={() => {
                setCreating(true);
                setChosen(false);
              }}
              className={`flex w-full items-center gap-2.5 rounded-[var(--radius-control)] border px-3 py-2 text-left transition-colors ${
                creating
                  ? "border-accent bg-accent-soft"
                  : "border-border hover:bg-surface"
              }`}
            >
              <FolderPlus
                className={`h-4 w-4 shrink-0 ${creating ? "text-accent" : "text-ink-faint"}`}
              />
              <span className={`text-sm ${creating ? "text-accent" : "text-ink"}`}>
                A new folder
              </span>
            </button>
          </div>

          {creating && (
            <div className="mt-3">
              <Field
                label="New folder name"
                autoFocus
                required
                maxLength={100}
                value={newName}
                onChange={(event) => setNewName(event.target.value)}
                placeholder="Invoices"
                hint="Created as a private Telegram channel."
              />
            </div>
          )}

          <Button
            busy={busy}
            disabled={!chosen && !creating}
            onClick={() => void go()}
            className="mt-4 w-full"
          >
            Move here
          </Button>

          <p className="mt-3 border-t border-border-soft pt-3 text-[11px] leading-relaxed text-ink-faint">
            Telegram has no move, so this copies and then removes the
            original — on its servers, so size costs nothing. The copy is a
            new message, so it takes today's date and shows as forwarded in
            the Telegram app. Nothing is deleted until the copy has arrived.
          </p>
        </Card>
      </div>
    </div>
  );
}
