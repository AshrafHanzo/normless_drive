import { useCallback, useEffect, useState } from "react";
import { ArrowLeft, Lock } from "lucide-react";
import { api, ApiError, type User } from "./api";
import { applyTheme, loadTheme, watchSystemTheme } from "./theme";
import { applyPreset, loadPreset } from "./themes";
import { Admin } from "./pages/Admin";
import { ConnectTelegram } from "./pages/ConnectTelegram";
import { Drive } from "./pages/Drive";
import { PublicShare } from "./pages/PublicShare";
import { SignIn } from "./pages/SignIn";
import { Alert, Button, Card, PasswordField, Spinner } from "./ui";

export default function App() {
  // Applied before anything meaningful renders so a reload does not flash
  // the wrong theme. index.html starts dark; this corrects it.
  useEffect(() => {
    // A preset overrides the light/dark choice entirely, so it is
    // applied second and wins.
    applyTheme(loadTheme());
    applyPreset(loadPreset());
    return watchSystemTheme(loadTheme);
  }, []);

  // A share link is a different application: no account, no chrome, and it
  // must not try to resolve a session. Decided first so opening one never
  // flashes the sign-in screen at a stranger.
  const shareToken = sharePathToken();
  if (shareToken) return <PublicShare token={shareToken} />;

  return <SignedInApp />;
}

/// Extracts the token from `/s/{token}`, or null for every other path.
function sharePathToken(): string | null {
  const match = window.location.pathname.match(/^\/s\/([A-Za-z0-9_-]+)\/?$/);
  return match?.[1] ?? null;
}

type Screen = "drive" | "admin";

function SignedInApp() {
  const [user, setUser] = useState<User | null>(null);
  const [checking, setChecking] = useState(true);
  const [screen, setScreen] = useState<Screen>("drive");

  // The session is an HttpOnly cookie, so asking the server is the only
  // way to know who we are — there is nothing readable from JavaScript.
  useEffect(() => {
    let cancelled = false;
    api
      .me()
      .then((me) => {
        if (!cancelled) setUser(me);
      })
      .catch((caught) => {
        // 401 just means "not signed in", the normal first visit.
        if (!(caught instanceof ApiError && caught.isAuthProblem)) {
          console.error("could not resolve session", caught);
        }
      })
      .finally(() => {
        if (!cancelled) setChecking(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const signOut = useCallback(async () => {
    await api.logout().catch(() => undefined);
    setUser(null);
    setScreen("drive");
  }, []);

  if (checking) return <Spinner />;
  if (!user) return <SignIn onSignedIn={setUser} />;

  if (screen === "admin" && user.role === "admin") {
    return (
      <div className="min-h-full bg-canvas">
        <div className="mx-auto max-w-5xl px-4 pt-6">
          <Button variant="ghost" onClick={() => setScreen("drive")}>
            <ArrowLeft className="h-3.5 w-3.5" />
            Back to drive
          </Button>
        </div>
        <Admin me={user} />
      </div>
    );
  }

  return (
    <TelegramGate
      user={user}
      onSignOut={signOut}
      onOpenAdmin={() => setScreen("admin")}
    />
  );
}

/// Shown when the cookie is still good but the key that decrypts this
/// user's Telegram session is gone — which is what a server restart does,
/// by design, since that key is never written to disk.
function Locked({ onUnlocked }: { onUnlocked: () => void }) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api.unlock(password);
      onUnlocked();
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "Could not unlock.",
      );
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-full items-center justify-center bg-canvas px-4 py-12">
      <div className="w-full max-w-sm">
        <div className="mb-5 text-center">
          <div className="mx-auto mb-3 flex h-11 w-11 items-center justify-center rounded-[var(--radius-card)] bg-accent-soft">
            <Lock className="h-5 w-5 text-accent" />
          </div>
          <h1 className="text-base font-semibold text-ink">Unlock your drive</h1>
          <p className="mx-auto mt-2 text-sm leading-relaxed text-ink-muted">
            You are still signed in. The key that decrypts your Telegram
            connection is kept only in memory and was cleared when the
            server restarted — that is deliberate, and it is what makes a
            stolen database useless.
          </p>
        </div>

        <Card className="p-5">
          <form onSubmit={submit} className="space-y-3">
            {error && <Alert tone="error">{error}</Alert>}
            <PasswordField
              label="Your password"
              autoFocus
              required
              value={password}
              onChange={(event) => setPassword(event.target.value)}
            />
            <Button type="submit" busy={busy} className="w-full">
              Unlock
            </Button>
          </form>
        </Card>
      </div>
    </div>
  );
}

/// Shows the connect wizard until Telegram is linked, then the drive.
function TelegramGate({
  user,
  onSignOut,
  onOpenAdmin,
}: {
  user: User;
  onSignOut: () => void;
  onOpenAdmin: () => void;
}) {
  const [connected, setConnected] = useState<boolean | null>(null);
  const [hint, setHint] = useState<string | null>(null);
  const [locked, setLocked] = useState(false);

  const refresh = useCallback(() => {
    api.telegram
      .status()
      .then((status) => {
        setConnected(status.connected);
        setHint(status.phone_hint);
        // Connected but not unlocked means the server restarted since
        // this person signed in. Everything Telegram-related will fail
        // until they sign in again, so say so instead of letting them
        // discover it one broken action at a time.
        // Needed whether or not Telegram is connected: connecting also
        // requires the key, so prompting only afterwards would fail the
        // very first attempt.
        setLocked(!status.unlocked);
      })
      .catch(() => setConnected(false));
  }, []);

  useEffect(refresh, [refresh]);

  if (connected === null) return <Spinner />;
  if (locked) return <Locked onUnlocked={refresh} />;
  if (!connected) return <ConnectTelegram onConnected={refresh} />;

  return (
    <Drive
      user={user}
      phoneHint={hint}
      onDisconnected={refresh}
      onSignOut={onSignOut}
      onOpenAdmin={onOpenAdmin}
    />
  );
}
