import { useEffect, useState } from "react";
import { Check, Monitor, Moon, RotateCcw, Sun, X } from "lucide-react";
import { api, ApiError } from "../api";
import { applyTheme, loadTheme, type Theme } from "../theme";
import { applyPreset, loadPreset, PRESETS } from "../themes";
import { loadZipPreference, saveZipPreference } from "../folderUpload";
import { Alert, Button, Card, PasswordField } from "../ui";

/// Settings that belong to the person, not to a file.
///
/// Appearance is local to the browser; offline sharing changes what the
/// server stores, so it asks for the password and explains itself.
export function Preferences({
  phoneHint,
  onClose,
}: {
  phoneHint: string | null;
  onClose: () => void;
}) {
  const [theme, setTheme] = useState<Theme>(loadTheme);
  const [preset, setPreset] = useState<string | null>(loadPreset);
  const [zipFolders, setZipFolders] = useState(loadZipPreference);
  const [offline, setOffline] = useState<boolean | null>(null);
  const [offlineAvailable, setOfflineAvailable] = useState(false);
  const [password, setPassword] = useState("");
  const [asking, setAsking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  useEffect(() => {
    api.shares
      .list()
      .then((result) => {
        setOfflineAvailable(result.offline_available);
        setOffline(result.shares.some((s) => s.allow_offline));
      })
      .catch(() => setOffline(false));
  }, []);

  function choose(next: Theme) {
    setTheme(next);
    applyTheme(next);
  }

  /// Presets and the light/dark switch are mutually exclusive: a preset
  /// fixes every colour, so leaving "System" highlighted underneath it
  /// would claim the app still follows the device when it does not.
  function pick(id: string | null) {
    setPreset(id);
    applyPreset(id);
    if (id === null) applyTheme(theme);
  }

  async function setOfflineSharing(enabled: boolean) {
    // Turning it on needs the password: the stored session has to be
    // decrypted to be re-wrapped, and it weakens the storage guarantee, so
    // it should not be a single unconsidered click.
    if (enabled && !password) {
      setAsking(true);
      return;
    }
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await api.shares.setOffline(enabled, password || undefined);
      setOffline(enabled);
      setAsking(false);
      setPassword("");
      setNotice(
        enabled
          ? "Share links will now keep working while you are signed out."
          : "Share links now work only while you are signed in.",
      );
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "Could not change that.",
      );
    } finally {
      setBusy(false);
    }
  }

  const themes: { value: Theme; label: string; icon: typeof Sun }[] = [
    { value: "system", label: "System", icon: Monitor },
    { value: "light", label: "Light", icon: Sun },
    { value: "dark", label: "Dark", icon: Moon },
  ];

  return (
    <div
      className="fixed inset-0 z-50 flex items-start justify-center overflow-y-auto bg-black/50 p-4 backdrop-blur-sm"
      onClick={onClose}
    >
      <div
        className="my-8 w-full max-w-md"
        onClick={(event) => event.stopPropagation()}
      >
        <Card className="p-5">
          <div className="mb-5 flex items-start justify-between gap-4">
            <h2 className="text-sm font-semibold text-ink">Preferences</h2>
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
          {notice && (
            <div className="mb-3">
              <Alert tone="success">{notice}</Alert>
            </div>
          )}

          <section className="mb-5">
            <h3 className="mb-2 text-xs font-medium text-ink-muted">
              Appearance
            </h3>
            <div className="grid grid-cols-3 gap-2">
              {themes.map(({ value, label, icon: Icon }) => (
                <button
                  key={value}
                  onClick={() => choose(value)}
                  className={`flex flex-col items-center gap-1.5 rounded-[var(--radius-control)]
                              border px-3 py-3 text-xs transition-colors ${
                                preset === null && theme === value
                                  ? "border-accent bg-accent-soft text-accent"
                                  : "border-border text-ink-muted hover:bg-surface hover:text-ink"
                              }`}
                >
                  <Icon className="h-4 w-4" />
                  {label}
                </button>
              ))}
            </div>
            <p className="mt-1.5 text-[11px] leading-relaxed text-ink-faint">
              System follows your device. A preset below overrides this.
            </p>
          </section>

          <section className="mb-5">
            <h3 className="mb-2 text-xs font-medium text-ink-muted">Presets</h3>
            <div className="grid grid-cols-2 gap-2">
              {PRESETS.map((item) => (
                <button
                  key={item.id}
                  onClick={() => pick(item.id)}
                  className={`overflow-hidden rounded-[var(--radius-control)] border text-left
                              transition-colors ${
                                preset === item.id
                                  ? "border-accent ring-1 ring-accent"
                                  : "border-border hover:border-accent/40"
                              }`}
                >
                  {/* Three bands: ground, raised, accent — the same way
                      the desktop app previews a theme, so a palette can
                      be judged without applying it. */}
                  <span className="flex h-10 w-full">
                    <span
                      className="h-full flex-1"
                      style={{ backgroundColor: item.swatch[0] }}
                    />
                    <span
                      className="h-full flex-1"
                      style={{ backgroundColor: item.swatch[1] }}
                    />
                    <span
                      className="h-full w-1/3"
                      style={{ backgroundColor: item.swatch[2] }}
                    />
                  </span>
                  <span className="flex items-center justify-between px-2.5 py-1.5">
                    <span className="truncate text-xs text-ink">{item.name}</span>
                    {preset === item.id && (
                      <Check className="h-3 w-3 shrink-0 text-accent" />
                    )}
                  </span>
                </button>
              ))}
            </div>
            {preset && (
              <Button
                variant="ghost"
                onClick={() => pick(null)}
                className="mt-2 w-full"
              >
                <RotateCcw className="h-3.5 w-3.5" />
                Back to the standard theme
              </Button>
            )}
          </section>

          <section className="mb-5 border-t border-border-soft pt-4">
            <h3 className="mb-2 text-xs font-medium text-ink-muted">Uploads</h3>
            <label className="flex cursor-pointer items-start justify-between gap-3 rounded-[var(--radius-control)] border border-border p-3">
              <span>
                <span className="block text-sm text-ink">
                  Zip folders before upload
                </span>
                <span className="mt-1 block text-xs leading-relaxed text-ink-faint">
                  {zipFolders
                    ? "A folder becomes one .zip. Structure is kept exactly, but you must download the whole archive to get one file out of it."
                    : "Every file is uploaded separately, each one downloadable on its own, with its path kept in the name."}
                </span>
              </span>
              <input
                type="checkbox"
                checked={zipFolders}
                onChange={(event) => {
                  setZipFolders(event.target.checked);
                  saveZipPreference(event.target.checked);
                }}
                className="mt-0.5 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
              />
            </label>
          </section>

          <section className="mb-5 border-t border-border-soft pt-4">
            <h3 className="mb-2 text-xs font-medium text-ink-muted">Sharing</h3>

            <div className="rounded-[var(--radius-control)] border border-border p-3">
              <div className="flex items-start justify-between gap-3">
                <div>
                  <p className="text-sm text-ink">
                    Keep share links working while I am signed out
                  </p>
                  <p className="mt-1 text-xs leading-relaxed text-ink-faint">
                    {offlineAvailable
                      ? "Without this, a link stops working the moment you sign out — the server needs your password to reach Telegram."
                      : "Not configured on this server."}
                  </p>
                </div>
                <input
                  type="checkbox"
                  disabled={!offlineAvailable || busy || offline === null}
                  checked={offline === true}
                  onChange={(event) => void setOfflineSharing(event.target.checked)}
                  className="mt-0.5 h-4 w-4 shrink-0 accent-[var(--color-accent)]"
                />
              </div>

              {asking && (
                <form
                  className="mt-3 space-y-2 border-t border-border-soft pt-3"
                  onSubmit={(event) => {
                    event.preventDefault();
                    void setOfflineSharing(true);
                  }}
                >
                  <PasswordField
                    label="Confirm your password"
                    autoFocus
                    required
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                  />
                  <Button type="submit" busy={busy} className="w-full">
                    Turn on
                  </Button>
                </form>
              )}

              {/* Stated plainly rather than buried. Someone enabling this
                  is widening what a server breach would expose, and they
                  deserve to know in the same breath as the switch. */}
              {offlineAvailable && (
                <p className="mt-3 border-t border-border-soft pt-3 text-xs leading-relaxed text-ink-faint">
                  <strong className="text-ink-muted">What this changes.</strong>{" "}
                  A second copy of your Telegram session is stored, encrypted
                  with a key held in the server's configuration rather than
                  derived from your password. A stolen database alone still
                  cannot read it; someone with the database <em>and</em> the
                  server's configuration could.
                </p>
              )}
            </div>
          </section>

          <section className="border-t border-border-soft pt-4">
            <h3 className="mb-2 text-xs font-medium text-ink-muted">Account</h3>
            <p className="text-xs text-ink-faint">
              Telegram connected{phoneHint ? ` · ${phoneHint}` : ""}
            </p>
          </section>
        </Card>
      </div>
    </div>
  );
}
