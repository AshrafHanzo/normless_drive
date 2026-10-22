import { useCallback, useEffect, useState } from "react";
import { ArrowLeft } from "lucide-react";
import { api, ApiError, type User } from "./api";
import { applyTheme, loadTheme, watchSystemTheme } from "./theme";
import { Admin } from "./pages/Admin";
import { ConnectTelegram } from "./pages/ConnectTelegram";
import { Drive } from "./pages/Drive";
import { PublicShare } from "./pages/PublicShare";
import { SignIn } from "./pages/SignIn";
import { Button, Spinner } from "./ui";

export default function App() {
  // Applied before anything meaningful renders so a reload does not flash
  // the wrong theme. index.html starts dark; this corrects it.
  useEffect(() => {
    applyTheme(loadTheme());
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

  const refresh = useCallback(() => {
    api.telegram
      .status()
      .then((status) => {
        setConnected(status.connected);
        setHint(status.phone_hint);
      })
      .catch(() => setConnected(false));
  }, []);

  useEffect(refresh, [refresh]);

  if (connected === null) return <Spinner />;
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
