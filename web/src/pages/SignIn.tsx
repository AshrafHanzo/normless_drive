import { useState } from "react";
import { HardDrive } from "lucide-react";
import { api, ApiError, type User } from "../api";
import { Alert, Button, Card, Field } from "../ui";

type Mode = "signin" | "register";

export function SignIn({ onSignedIn }: { onSignedIn: (user: User) => void }) {
  const [mode, setMode] = useState<Mode>("signin");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  async function submit(event: React.FormEvent) {
    event.preventDefault();
    setBusy(true);
    setError(null);
    setNotice(null);

    try {
      if (mode === "register") {
        const result = await api.register(email, password);
        // A new account is 'pending' unless it is the configured admin, so
        // say what happens next rather than dropping them at a login form
        // that will refuse them.
        if (result.user.status === "active") {
          onSignedIn(await api.login(email, password));
          return;
        }
        setNotice(result.message);
        setMode("signin");
      } else {
        onSignedIn(await api.login(email, password));
      }
    } catch (caught) {
      setError(
        caught instanceof ApiError
          ? caught.message
          : "Something went wrong. Please try again.",
      );
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="flex min-h-full items-center justify-center bg-canvas px-4 py-12">
      <div className="w-full max-w-sm">
        <div className="mb-8 flex flex-col items-center text-center">
          <div className="mb-3 flex h-11 w-11 items-center justify-center rounded-[var(--radius-card)] bg-accent">
            <HardDrive className="h-5 w-5 text-white" />
          </div>
          <h1 className="text-lg font-semibold text-ink">Normless Drive</h1>
          <p className="mt-1 text-sm text-ink-muted">
            {mode === "signin"
              ? "Sign in to your account"
              : "Request access to this drive"}
          </p>
        </div>

        <Card className="p-5">
          <form onSubmit={submit} className="space-y-4">
            {error && <Alert tone="error">{error}</Alert>}
            {notice && <Alert tone="success">{notice}</Alert>}

            <Field
              label="Email"
              type="email"
              autoComplete="email"
              required
              value={email}
              onChange={(e) => setEmail(e.target.value)}
              placeholder="you@example.com"
            />

            <Field
              label="Password"
              type="password"
              autoComplete={
                mode === "signin" ? "current-password" : "new-password"
              }
              required
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              placeholder="••••••••••••"
              hint={
                mode === "register" ? "At least 12 characters." : undefined
              }
            />

            <Button type="submit" busy={busy} className="w-full">
              {mode === "signin" ? "Sign in" : "Create account"}
            </Button>
          </form>

          <div className="mt-4 border-t border-border-soft pt-4 text-center">
            <button
              type="button"
              onClick={() => {
                setMode(mode === "signin" ? "register" : "signin");
                setError(null);
                setNotice(null);
              }}
              className="text-xs text-accent hover:underline"
            >
              {mode === "signin"
                ? "Need an account? Request access"
                : "Already have an account? Sign in"}
            </button>
          </div>
        </Card>

        {mode === "register" && (
          <p className="mt-4 text-center text-xs leading-relaxed text-ink-faint">
            New accounts need an administrator's approval before they can
            sign in.
          </p>
        )}
      </div>
    </div>
  );
}
