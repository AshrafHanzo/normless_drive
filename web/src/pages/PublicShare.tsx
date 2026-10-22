import { useCallback, useEffect, useState } from "react";
import { Download, HardDrive, Lock } from "lucide-react";
import { api, ApiError, type FileRow, type SharePermissions } from "../api";
import { Alert, Button, Card, PasswordField, Spinner } from "../ui";

/// What someone who was handed a link sees.
///
/// Deliberately standalone: no header, no account, nothing that implies
/// they need to sign in. Most people opening one of these have never heard
/// of this service and should not have to.
export function PublicShare({ token }: { token: string }) {
  const [state, setState] = useState<
    | { kind: "loading" }
    | { kind: "gone"; message: string }
    | {
        kind: "locked";
        label: string | null;
      }
    | {
        kind: "open";
        label: string | null;
        permissions: SharePermissions;
        available: boolean;
        files: FileRow[] | null;
      }
  >({ kind: "loading" });

  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const open = useCallback(async () => {
    try {
      const info = await api.publicShare.info(token);
      if (info.needs_password && !info.unlocked) {
        setState({ kind: "locked", label: info.label });
        return;
      }
      setState({
        kind: "open",
        label: info.label,
        permissions: info.permissions,
        available: info.available,
        files: null,
      });

      if (info.available && info.permissions.can_download) {
        const listed = await api.publicShare.files(token);
        setState((previous) =>
          previous.kind === "open"
            ? { ...previous, files: listed.files }
            : previous,
        );
      }
    } catch (caught) {
      setState({
        kind: "gone",
        message:
          caught instanceof ApiError
            ? caught.message
            : "This link is not available.",
      });
    }
  }, [token]);

  useEffect(() => {
    void open();
  }, [open]);

  async function unlock(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.publicShare.verify(token, password);
      await open();
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "Could not unlock.",
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-full items-start justify-center bg-canvas px-4 py-12">
      <div className="w-full max-w-2xl">
        <div className="mb-6 flex items-center justify-center gap-2.5">
          <div className="flex h-8 w-8 items-center justify-center rounded-[var(--radius-control)] bg-accent">
            <HardDrive className="h-4 w-4 text-white" />
          </div>
          <span className="text-sm font-semibold text-ink">Normless Drive</span>
        </div>

        {state.kind === "loading" && <Spinner />}

        {state.kind === "gone" && (
          <Card className="px-6 py-14 text-center">
            <p className="text-base font-semibold text-ink">
              This link is not available
            </p>
            <p className="mx-auto mt-2 max-w-sm text-sm text-ink-muted">
              {state.message} It may have expired, been used up, or been
              revoked by whoever shared it.
            </p>
          </Card>
        )}

        {state.kind === "locked" && (
          <Card className="mx-auto max-w-sm p-6">
            <div className="mb-4 flex flex-col items-center text-center">
              <Lock className="mb-2 h-5 w-5 text-ink-faint" />
              <p className="text-sm font-semibold text-ink">
                {state.label ?? "Protected link"}
              </p>
              <p className="mt-1 text-xs text-ink-muted">
                Enter the password you were given.
              </p>
            </div>
            <form onSubmit={unlock} className="space-y-3">
              {error && <Alert tone="error">{error}</Alert>}
              <PasswordField
                label="Password"
                autoFocus
                required
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
              <Button type="submit" busy={busy} className="w-full">
                Unlock
              </Button>
            </form>
          </Card>
        )}

        {state.kind === "open" && (
          <>
            <div className="mb-4 text-center">
              <h1 className="text-lg font-semibold text-ink">
                {state.label ?? "Shared files"}
              </h1>
              <p className="mt-0.5 text-xs text-ink-muted">
                {describe(state.permissions)}
              </p>
            </div>

            {!state.available && (
              <div className="mb-4">
                <Alert tone="warn">
                  These files cannot be reached right now. This link only works
                  while its owner is signed in, and they are not at the moment.
                </Alert>
              </div>
            )}

            {!state.permissions.can_download ? (
              <Card className="px-6 py-14 text-center">
                <p className="text-sm text-ink-muted">
                  This link is for uploading only — you cannot see what is
                  already here.
                </p>
              </Card>
            ) : state.files === null ? (
              state.available ? (
                <Spinner />
              ) : null
            ) : state.files.length === 0 ? (
              <Card className="px-6 py-14 text-center">
                <p className="text-sm text-ink-muted">Nothing shared here yet.</p>
              </Card>
            ) : (
              <Card className="divide-y divide-border-soft">
                {state.files.map((file) => (
                  <div key={file.id} className="flex items-center gap-3 px-4 py-3">
                    <div className="min-w-0 flex-1">
                      <p className="truncate text-sm text-ink" title={file.name}>
                        {file.name}
                      </p>
                      <p className="text-xs text-ink-faint">
                        {formatSize(file.size)}
                      </p>
                    </div>
                    <a
                      href={api.publicShare.downloadUrl(token, file.id)}
                      className="flex items-center gap-1.5 rounded-[var(--radius-control)]
                                 border border-border px-2.5 py-1.5 text-xs text-ink-muted
                                 transition-colors hover:bg-surface hover:text-ink"
                    >
                      <Download className="h-3.5 w-3.5" />
                      Download
                    </a>
                  </div>
                ))}
              </Card>
            )}
          </>
        )}
      </div>
    </div>
  );
}

function describe(permissions: SharePermissions) {
  const allowed = [
    permissions.can_download && "download",
    permissions.can_upload && "upload",
    permissions.can_delete && "delete",
  ].filter(Boolean);
  return allowed.length ? `You can ${allowed.join(" and ")}.` : "Read only.";
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
