import { useCallback, useEffect, useState } from "react";
import { HardDrive, LogOut, Shield } from "lucide-react";
import { api, ApiError, type User } from "./api";
import { Admin } from "./pages/Admin";
import { ConnectTelegram } from "./pages/ConnectTelegram";
import { Files } from "./pages/Files";
import { SignIn } from "./pages/SignIn";
import { Button, Spinner } from "./ui";

type View = "drive" | "admin";

export default function App() {
  const [user, setUser] = useState<User | null>(null);
  const [checking, setChecking] = useState(true);
  const [view, setView] = useState<View>("drive");

  // Ask the server who we are on load. The session is an HttpOnly cookie, so
  // this is the only way to know — there is nothing readable in JS to check.
  useEffect(() => {
    let cancelled = false;
    api
      .me()
      .then((me) => {
        if (!cancelled) setUser(me);
      })
      .catch((caught) => {
        // 401 just means "not signed in", which is the normal first visit.
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

  if (checking) return <Spinner />;
  if (!user) return <SignIn onSignedIn={setUser} />;

  return (
    <div className="min-h-full bg-canvas">
      <header className="border-b border-border bg-surface">
        <div className="mx-auto flex max-w-5xl items-center justify-between gap-4 px-4 py-3">
          <div className="flex items-center gap-2.5">
            <div className="flex h-7 w-7 items-center justify-center rounded-[var(--radius-control)] bg-accent">
              <HardDrive className="h-4 w-4 text-white" />
            </div>
            <span className="text-sm font-semibold text-ink">
              Normless Drive
            </span>
          </div>

          <div className="flex items-center gap-2">
            {user.role === "admin" && (
              <Button
                variant="ghost"
                onClick={() => setView(view === "admin" ? "drive" : "admin")}
              >
                <Shield className="h-3.5 w-3.5" />
                {view === "admin" ? "Back to drive" : "Admin"}
              </Button>
            )}
            <span className="hidden text-xs text-ink-muted sm:inline">
              {user.email}
            </span>
            <Button
              variant="ghost"
              onClick={async () => {
                await api.logout().catch(() => undefined);
                setUser(null);
                setView("drive");
              }}
            >
              <LogOut className="h-3.5 w-3.5" />
            </Button>
          </div>
        </div>
      </header>

      {view === "admin" && user.role === "admin" ? (
        <Admin me={user} />
      ) : (
        <Drive />
      )}
    </div>
  );
}

/// Shows the connect wizard until Telegram is linked, then the drive.
function Drive() {
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

  return <Files phoneHint={hint} onDisconnected={refresh} />;
}

