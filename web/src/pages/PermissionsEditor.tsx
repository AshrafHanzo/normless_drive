import { useEffect, useState } from "react";
import { X } from "lucide-react";
import {
  api,
  ApiError,
  type AdminUserRow,
  type Permissions,
} from "../api";
import { Alert, Button, Card, Spinner } from "../ui";

/// Per-account feature switches and storage cap.
///
/// Loads on open rather than alongside the user list: most of the time
/// nobody is editing permissions, and fetching them per row would mean a
/// query per user on every page load.
export function PermissionsEditor({
  user,
  onClose,
  onSaved,
}: {
  user: AdminUserRow;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [limits, setLimits] = useState<Permissions | null>(null);
  const [usedBytes, setUsedBytes] = useState(0);
  const [quotaGb, setQuotaGb] = useState("0");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.admin
      .permissions(user.id)
      .then((result) => {
        setLimits(result.permissions);
        setUsedBytes(result.used_bytes);
        setQuotaGb((result.permissions.storage_quota_bytes / 1024 ** 3).toFixed(0));
      })
      .catch((caught) =>
        setError(
          caught instanceof ApiError
            ? caught.message
            : "Could not load permissions.",
        ),
      );
  }, [user.id]);

  async function save() {
    if (!limits) return;
    setBusy(true);
    setError(null);
    try {
      const gb = Math.max(0, parseFloat(quotaGb) || 0);
      await api.admin.setPermissions(user.id, {
        ...limits,
        storage_quota_bytes: Math.round(gb * 1024 ** 3),
      });
      onSaved();
    } catch (caught) {
      setError(caught instanceof ApiError ? caught.message : "Could not save.");
      setBusy(false);
    }
  }

  const toggles: { key: keyof Permissions; label: string; hint: string }[] = [
    { key: "can_upload", label: "Upload files", hint: "Add files to their drive" },
    { key: "can_download", label: "Download files", hint: "Retrieve their files" },
    { key: "can_delete", label: "Delete files", hint: "Remove files permanently" },
    {
      key: "can_create_folders",
      label: "Create folders",
      hint: "Make new Telegram channels",
    },
    { key: "can_share", label: "Create share links", hint: "Share files publicly" },
  ];

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-black/50 p-4 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="max-h-[85vh] w-full max-w-md overflow-y-auto"
        // Without this, a click on anything inside bubbles out and closes
        // the dialog mid-edit.
        onClick={(event) => event.stopPropagation()}
      >
        <Card className="p-5">
          <div className="mb-4 flex items-start justify-between gap-4">
            <div>
              <h2 className="text-sm font-semibold text-ink">Permissions</h2>
              <p className="text-xs text-ink-muted">{user.email}</p>
            </div>
            <button
              onClick={onClose}
              aria-label="Close"
              className="text-ink-faint transition-colors hover:text-ink"
            >
              <X className="h-4 w-4" />
            </button>
          </div>

          {error && (
            <div className="mb-3">
              <Alert tone="error">{error}</Alert>
            </div>
          )}

          {!limits ? (
            <Spinner />
          ) : (
            <>
              <div className="space-y-1">
                {toggles.map(({ key, label, hint }) => (
                  <label
                    key={key}
                    className="flex cursor-pointer items-center justify-between gap-4 rounded-[var(--radius-control)] px-2 py-2 hover:bg-surface"
                  >
                    <span>
                      <span className="block text-sm text-ink">{label}</span>
                      <span className="block text-xs text-ink-faint">{hint}</span>
                    </span>
                    <input
                      type="checkbox"
                      checked={Boolean(limits[key])}
                      onChange={(event) =>
                        setLimits({ ...limits, [key]: event.target.checked })
                      }
                      className="h-4 w-4 accent-[var(--color-accent)]"
                    />
                  </label>
                ))}
              </div>

              <div className="mt-4 border-t border-border-soft pt-4">
                <label className="block">
                  <span className="mb-1.5 block text-xs font-medium text-ink-muted">
                    Storage limit (GB)
                  </span>
                  <input
                    type="number"
                    min="0"
                    value={quotaGb}
                    onChange={(event) => setQuotaGb(event.target.value)}
                    className="w-28 rounded-[var(--radius-control)] border border-border
                               bg-canvas px-3 py-2 text-sm text-ink outline-none focus:border-accent"
                  />
                  <span className="mt-1 block text-xs text-ink-faint">
                    0 means no limit. Currently using{" "}
                    {(usedBytes / 1024 ** 3).toFixed(2)} GB.
                  </span>
                </label>
              </div>

              <div className="mt-5 flex gap-2">
                <Button variant="ghost" onClick={onClose} className="flex-1">
                  Cancel
                </Button>
                <Button busy={busy} onClick={() => void save()} className="flex-1">
                  Save
                </Button>
              </div>
            </>
          )}
        </Card>
      </div>
    </div>
  );
}
