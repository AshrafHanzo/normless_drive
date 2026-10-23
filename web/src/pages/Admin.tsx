import { useCallback, useEffect, useMemo, useState } from "react";
import {
  Ban,
  Check,
  Clock,
  RotateCw,
  Search,
  ShieldCheck,
  ShieldOff,
  SlidersHorizontal,
  UserCheck,
  Users,
} from "lucide-react";
import {
  api,
  ApiError,
  type AdminUserRow,
  type AuditEntry,
  type Role,
  type Status,
  type User,
} from "../api";
import { Alert, Badge, Button, Card, Spinner } from "../ui";
import { PermissionsEditor } from "./PermissionsEditor";

export function Admin({ me }: { me: User }) {
  const [users, setUsers] = useState<AdminUserRow[] | null>(null);
  const [audit, setAudit] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Per-row, so one slow request does not disable every button on the page.
  const [pendingRow, setPendingRow] = useState<string | null>(null);
  const [editing, setEditing] = useState<AdminUserRow | null>(null);
  const [query, setQuery] = useState("");

  const load = useCallback(async () => {
    setError(null);
    try {
      const [userList, auditList] = await Promise.all([
        api.admin.users(),
        api.admin.audit(50),
      ]);
      setUsers(userList.users);
      setAudit(auditList.entries);
    } catch (caught) {
      setError(
        caught instanceof ApiError ? caught.message : "Could not load users.",
      );
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function act(id: string, run: () => Promise<unknown>) {
    setPendingRow(id);
    setError(null);
    try {
      await run();
      await load();
    } catch (caught) {
      setError(caught instanceof ApiError ? caught.message : "That did not work.");
    } finally {
      setPendingRow(null);
    }
  }

  const setStatus = (id: string, status: Status) =>
    act(id, () => api.admin.setStatus(id, status));
  const setRole = (id: string, role: Role) =>
    act(id, () => api.admin.setRole(id, role));

  const shown = useMemo(() => {
    if (!users) return null;
    const needle = query.trim().toLowerCase();
    return needle
      ? users.filter((u) => u.email.toLowerCase().includes(needle))
      : users;
  }, [users, query]);

  if (!users) return <Spinner />;

  const pending = users.filter((u) => u.status === "pending").length;
  const active = users.filter((u) => u.status === "active").length;
  const connected = users.filter((u) => u.telegram_connected).length;

  return (
    <div className="mx-auto max-w-5xl space-y-5 px-4 py-6">
      <header>
        <h1 className="text-xl font-semibold tracking-tight text-ink">
          Administration
        </h1>
        <p className="mt-0.5 text-sm text-ink-muted">
          Who may use this drive, and what they are allowed to do.
        </p>
      </header>

      {/* Counts first. The number that matters is how many people are
          waiting on you — everything else is context. */}
      <div className="grid grid-cols-2 gap-3 sm:grid-cols-4">
        <Stat
          icon={Clock}
          label="Waiting for approval"
          value={pending}
          tone={pending > 0 ? "warn" : "plain"}
        />
        <Stat icon={UserCheck} label="Active" value={active} tone="plain" />
        <Stat icon={Users} label="Accounts" value={users.length} tone="plain" />
        <Stat
          icon={ShieldCheck}
          label="Telegram linked"
          value={connected}
          tone="plain"
        />
      </div>

      {error && <Alert tone="error">{error}</Alert>}

      <div className="flex flex-wrap items-center gap-2">
        <div className="relative min-w-0 flex-1">
          <Search className="pointer-events-none absolute left-3 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-ink-faint" />
          <input
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Search accounts…"
            className="w-full rounded-full border border-border bg-canvas py-1.5 pl-9 pr-3
                       text-sm text-ink outline-none transition-colors
                       placeholder:text-ink-faint focus:border-accent"
          />
        </div>
        <Button variant="ghost" onClick={() => void load()}>
          <RotateCw className="h-3.5 w-3.5" />
          Refresh
        </Button>
      </div>

      <Card className="overflow-hidden">
        {shown && shown.length === 0 ? (
          <p className="px-4 py-12 text-center text-sm text-ink-faint">
            No accounts match.
          </p>
        ) : (
          <div className="divide-y divide-border-soft">
            {shown?.map((user) => {
              const isSelf = user.id === me.id;
              const busy = pendingRow === user.id;
              return (
                <div
                  key={user.id}
                  className="flex flex-wrap items-center gap-3 px-4 py-3 transition-colors hover:bg-surface"
                >
                  <Avatar email={user.email} />

                  <div className="min-w-0 flex-1">
                    <p className="truncate text-sm font-medium text-ink">
                      {user.email}
                      {isSelf && (
                        <span className="ml-1.5 text-xs font-normal text-ink-faint">
                          you
                        </span>
                      )}
                    </p>
                    <p className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-ink-faint">
                      <span>
                        {user.telegram_connected
                          ? "Telegram linked"
                          : "No Telegram yet"}
                      </span>
                      <span aria-hidden>·</span>
                      <span>
                        {user.last_login_at
                          ? `seen ${new Date(user.last_login_at).toLocaleDateString()}`
                          : "never signed in"}
                      </span>
                    </p>
                  </div>

                  <div className="flex shrink-0 items-center gap-1.5">
                    <Badge tone={user.status}>{user.status}</Badge>
                    <Badge tone={user.role}>{user.role}</Badge>
                  </div>

                  <div className="flex shrink-0 items-center gap-1">
                    {user.status !== "active" && (
                      <Button
                        busy={busy}
                        onClick={() => setStatus(user.id, "active")}
                        title="Allow this account to sign in"
                      >
                        <Check className="h-3.5 w-3.5" />
                        Approve
                      </Button>
                    )}
                    <Action
                      icon={SlidersHorizontal}
                      label="Permissions and storage limit"
                      busy={busy}
                      onClick={() => setEditing(user)}
                    />
                    {user.status === "active" && !isSelf && (
                      <Action
                        icon={Ban}
                        label="Suspend"
                        danger
                        busy={busy}
                        onClick={() => setStatus(user.id, "suspended")}
                      />
                    )}
                    {user.role === "user" ? (
                      <Action
                        icon={ShieldCheck}
                        label="Make administrator"
                        busy={busy}
                        onClick={() => setRole(user.id, "admin")}
                      />
                    ) : (
                      !isSelf && (
                        <Action
                          icon={ShieldOff}
                          label="Remove administrator"
                          busy={busy}
                          onClick={() => setRole(user.id, "user")}
                        />
                      )
                    )}
                  </div>
                </div>
              );
            })}
          </div>
        )}
      </Card>

      {editing && (
        <PermissionsEditor
          user={editing}
          onClose={() => setEditing(null)}
          onSaved={() => {
            setEditing(null);
            void load();
          }}
        />
      )}

      <section>
        <h2 className="mb-2 text-sm font-semibold text-ink">Recent activity</h2>
        <Card className="divide-y divide-border-soft">
          {audit.length === 0 && (
            <p className="px-4 py-8 text-center text-sm text-ink-faint">
              Nothing yet.
            </p>
          )}
          {audit.map((entry) => (
            <div
              key={entry.id}
              className="flex items-center justify-between gap-4 px-4 py-2.5"
            >
              <span className="flex min-w-0 items-center gap-2">
                <span
                  className={`h-1.5 w-1.5 shrink-0 rounded-full ${dotFor(entry.action)}`}
                />
                <span className="truncate text-sm text-ink-muted">
                  {describe(entry.action)}
                </span>
              </span>
              <span className="shrink-0 text-xs text-ink-faint">
                {new Date(entry.created_at).toLocaleString()}
              </span>
            </div>
          ))}
        </Card>
      </section>
    </div>
  );
}

function Stat({
  icon: Icon,
  label,
  value,
  tone,
}: {
  icon: typeof Users;
  label: string;
  value: number;
  tone: "plain" | "warn";
}) {
  return (
    <div
      className={`rounded-[var(--radius-card)] border p-3 ${
        tone === "warn" && value > 0
          ? "border-[var(--color-warn)]/30 bg-warn-soft"
          : "border-border bg-raised"
      }`}
    >
      <Icon
        className={`mb-2 h-4 w-4 ${
          tone === "warn" && value > 0 ? "text-warn" : "text-ink-faint"
        }`}
      />
      <p className="text-xl font-semibold tabular-nums text-ink">{value}</p>
      <p className="mt-0.5 text-xs text-ink-muted">{label}</p>
    </div>
  );
}

/// Initial on a colour derived from the address.
///
/// Deterministic rather than random so the same person keeps the same
/// colour between loads, which is what makes a list scannable at all.
function Avatar({ email }: { email: string }) {
  const palette = [
    "#2d7ff9",
    "#1a8a52",
    "#b06a00",
    "#d4493b",
    "#7c4dff",
    "#0097a7",
  ];
  let hash = 0;
  for (const char of email) hash = (hash * 31 + char.charCodeAt(0)) | 0;
  const colour = palette[Math.abs(hash) % palette.length];

  return (
    <span
      className="flex h-8 w-8 shrink-0 items-center justify-center rounded-full text-xs font-semibold text-white"
      style={{ backgroundColor: colour }}
      aria-hidden
    >
      {email[0]?.toUpperCase() ?? "?"}
    </span>
  );
}

function Action({
  icon: Icon,
  label,
  busy,
  danger = false,
  onClick,
}: {
  icon: typeof Ban;
  label: string;
  busy: boolean;
  danger?: boolean;
  onClick: () => void;
}) {
  return (
    <button
      onClick={onClick}
      disabled={busy}
      title={label}
      aria-label={label}
      className={`rounded-[var(--radius-control)] border border-border p-2 transition-colors
                  disabled:opacity-40 ${
                    danger
                      ? "text-ink-faint hover:bg-[var(--color-danger-soft)] hover:text-[var(--color-danger)]"
                      : "text-ink-faint hover:bg-surface hover:text-ink"
                  }`}
    >
      <Icon className="h-3.5 w-3.5" />
    </button>
  );
}

/// Audit actions are machine names; this renders them as English.
function describe(action: string): string {
  const map: Record<string, string> = {
    "user.register": "An account was created",
    "user.login": "Someone signed in",
    "user.login_failed": "A sign-in was refused",
    "user.password_changed": "A password was changed",
    "admin.set_status": "An account's access was changed",
    "admin.set_permissions": "Permissions were changed",
    "admin.set_role": "An administrator role was changed",
    "telegram.connected": "A Telegram account was linked",
    "telegram.disconnect": "A Telegram account was unlinked",
    "share.created": "A share link was created",
    "share.revoked": "A share link was revoked",
    "share.offline_enabled": "Offline sharing was turned on",
    "share.offline_disabled": "Offline sharing was turned off",
  };
  return map[action] ?? action;
}

function dotFor(action: string): string {
  if (action.includes("failed") || action.includes("revoked"))
    return "bg-[var(--color-danger)]";
  if (action.startsWith("admin.")) return "bg-[var(--color-warn)]";
  if (action.includes("connected") || action.includes("register"))
    return "bg-[var(--color-success)]";
  return "bg-[var(--color-ink-faint)]";
}
