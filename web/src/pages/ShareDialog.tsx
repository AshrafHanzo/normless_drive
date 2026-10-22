import { useEffect, useState } from "react";
import { Check, Copy, Link2, Trash2, X } from "lucide-react";
import { api, ApiError, type Share, type SharePermissions } from "../api";
import { Alert, Button, Card, Field, PasswordField, Spinner } from "../ui";

/// Create and manage share links for one folder.
///
/// Shows existing links alongside the form on purpose: the most common
/// reason to open this is to copy a link made earlier, not to make another.
export function ShareDialog({
  folderId,
  folderName,
  onClose,
}: {
  folderId: string | undefined;
  folderName: string;
  onClose: () => void;
}) {
  const [shares, setShares] = useState<Share[] | null>(null);
  const [offlineAvailable, setOfflineAvailable] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);

  const [permissions, setPermissions] = useState<SharePermissions>({
    can_download: true,
    can_upload: false,
    can_update: false,
    can_delete: false,
  });
  const [label, setLabel] = useState("");
  const [password, setPassword] = useState("");
  const [expiryHours, setExpiryHours] = useState("");
  const [usageLimit, setUsageLimit] = useState("");
  const [allowOffline, setAllowOffline] = useState(false);

  async function load() {
    try {
      const result = await api.shares.list();
      setShares(result.shares.filter((s) => (s.folder_id ?? undefined) === folderId));
      setOfflineAvailable(result.offline_available);
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "Could not load links.",
      );
      setShares([]);
    }
  }

  useEffect(() => {
    void load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [folderId]);

  async function create() {
    setBusy(true);
    setError(null);
    try {
      await api.shares.create({
        label: label.trim() || undefined,
        folder_id: folderId,
        permissions,
        password: password || undefined,
        expiry_hours: expiryHours ? Number(expiryHours) : undefined,
        usage_limit: usageLimit ? Number(usageLimit) : undefined,
        allow_offline: allowOffline,
      });
      setLabel("");
      setPassword("");
      setExpiryHours("");
      setUsageLimit("");
      await load();
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "Could not create the link.",
      );
    } finally {
      setBusy(false);
    }
  }

  async function copy(url: string, id: string) {
    try {
      await navigator.clipboard.writeText(url);
      setCopied(id);
      window.setTimeout(() => setCopied(null), 1500);
    } catch {
      // Clipboard access can be refused; selecting the text still works.
      setError("Could not copy. Select the link and copy it manually.");
    }
  }

  const toggles: { key: keyof SharePermissions; label: string }[] = [
    { key: "can_download", label: "Download files" },
    { key: "can_upload", label: "Upload files" },
    { key: "can_delete", label: "Delete files" },
  ];

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/50 p-4 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="my-8 w-full max-w-lg"
        onClick={(event) => event.stopPropagation()}
      >
        <Card className="p-5">
          <div className="mb-4 flex items-start justify-between gap-4">
            <div>
              <h2 className="text-sm font-semibold text-ink">Share</h2>
              <p className="text-xs text-ink-muted">{folderName}</p>
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

          <div className="space-y-3">
            <Field
              label="Label (optional)"
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              placeholder="Client intake"
              hint="Only you see this — it helps tell links apart."
            />

            <div>
              <span className="mb-1.5 block text-xs font-medium text-ink-muted">
                What the link allows
              </span>
              <div className="space-y-1">
                {toggles.map(({ key, label: text }) => (
                  <label
                    key={key}
                    className="flex cursor-pointer items-center justify-between rounded-[var(--radius-control)] px-2 py-1.5 text-sm text-ink hover:bg-surface"
                  >
                    {text}
                    <input
                      type="checkbox"
                      checked={permissions[key]}
                      onChange={(e) =>
                        setPermissions({ ...permissions, [key]: e.target.checked })
                      }
                      className="h-4 w-4 accent-[var(--color-accent)]"
                    />
                  </label>
                ))}
              </div>
            </div>

            <PasswordField
              label="Password (optional)"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              hint="Anyone opening the link must type this."
            />

            <div className="grid grid-cols-2 gap-3">
              <Field
                label="Expires after (hours)"
                type="number"
                min="1"
                value={expiryHours}
                onChange={(e) => setExpiryHours(e.target.value)}
                placeholder="Never"
              />
              <Field
                label="Max uses"
                type="number"
                min="1"
                value={usageLimit}
                onChange={(e) => setUsageLimit(e.target.value)}
                placeholder="Unlimited"
              />
            </div>

            <label
              className={`flex items-start justify-between gap-3 rounded-[var(--radius-control)] border border-border p-3 ${
                offlineAvailable ? "cursor-pointer" : "opacity-60"
              }`}
            >
              <span>
                <span className="block text-sm text-ink">
                  Keep working while I am signed out
                </span>
                <span className="mt-0.5 block text-xs leading-relaxed text-ink-faint">
                  {offlineAvailable
                    ? "Needs turning on once in settings. Without it the link only works while you have this site open."
                    : "Not configured on this server."}
                </span>
              </span>
              <input
                type="checkbox"
                disabled={!offlineAvailable}
                checked={allowOffline}
                onChange={(e) => setAllowOffline(e.target.checked)}
                className="mt-0.5 h-4 w-4 accent-[var(--color-accent)]"
              />
            </label>

            <Button busy={busy} onClick={() => void create()} className="w-full">
              <Link2 className="h-3.5 w-3.5" />
              Create link
            </Button>
          </div>

          <div className="mt-5 border-t border-border-soft pt-4">
            <h3 className="mb-2 text-xs font-medium text-ink-muted">
              Existing links
            </h3>
            {shares === null ? (
              <Spinner />
            ) : shares.length === 0 ? (
              <p className="py-3 text-xs text-ink-faint">None yet.</p>
            ) : (
              <div className="space-y-2">
                {shares.map((share) => (
                  <div
                    key={share.id}
                    className="rounded-[var(--radius-control)] border border-border p-2.5"
                  >
                    <div className="flex items-center gap-2">
                      <code className="min-w-0 flex-1 truncate text-xs text-ink-muted">
                        {share.url}
                      </code>
                      <button
                        onClick={() => void copy(share.url, share.id)}
                        aria-label="Copy link"
                        className="text-ink-faint transition-colors hover:text-ink"
                      >
                        {copied === share.id ? (
                          <Check className="h-3.5 w-3.5 text-[var(--color-success)]" />
                        ) : (
                          <Copy className="h-3.5 w-3.5" />
                        )}
                      </button>
                      <button
                        onClick={async () => {
                          await api.shares.revoke(share.id).catch(() => undefined);
                          await load();
                        }}
                        aria-label="Revoke link"
                        className="text-ink-faint transition-colors hover:text-[var(--color-danger)]"
                      >
                        <Trash2 className="h-3.5 w-3.5" />
                      </button>
                    </div>
                    <p className="mt-1 text-[11px] text-ink-faint">
                      {summarise(share)}
                    </p>
                  </div>
                ))}
              </div>
            )}
          </div>
        </Card>
      </div>
    </div>
  );
}

function summarise(share: Share) {
  const parts: string[] = [];
  if (share.label) parts.push(share.label);
  const allowed = [
    share.permissions.can_download && "download",
    share.permissions.can_upload && "upload",
    share.permissions.can_delete && "delete",
  ].filter(Boolean);
  parts.push(allowed.length ? allowed.join(" + ") : "no access");
  if (share.has_password) parts.push("password");
  if (share.allow_offline) parts.push("works offline");
  if (share.expires_at)
    parts.push(`expires ${new Date(share.expires_at).toLocaleDateString()}`);
  if (share.usage_limit)
    parts.push(`${share.usage_count}/${share.usage_limit} uses`);
  else if (share.usage_count) parts.push(`${share.usage_count} uses`);
  return parts.join(" · ");
}
