import { useCallback, useEffect, useState } from "react";
import { Check, Ban, ShieldCheck, ShieldOff, RotateCw } from "lucide-react";
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

export function Admin({ me }: { me: User }) {
  const [users, setUsers] = useState<AdminUserRow[] | null>(null);
  const [audit, setAudit] = useState<AuditEntry[]>([]);
  const [error, setError] = useState<string | null>(null);
  // Per-row, so one slow request does not disable every button on the page.
  const [pendingRow, setPendingRow] = useState<string | null>(null);

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
      setError(
        caught instanceof ApiError ? caught.message : "That did not work.",
      );
    } finally {
      setPendingRow(null);
    }
  }

  const setStatus = (id: string, status: Status) =>
    act(id, () => api.admin.setStatus(id, status));
  const setRole = (id: string, role: Role) =>
    act(id, () => api.admin.setRole(id, role));

  if (!users) return <Spinner />;

  const pending = users.filter((u) => u.status === "pending");

  return (
    <div className="mx-auto max-w-5xl space-y-6 px-4 py-8">
      <header className="flex flex-wrap items-center justify-between gap-3">
        <div>
          <h1 className="text-lg font-semibold text-ink">Administration</h1>
          <p className="text-sm text-ink-muted">
            {users.length} account{users.length === 1 ? "" : "s"}
            {pending.length > 0 && (
              <span className="text-warn">
                {" "}
                · {pending.length} waiting for approval
              </span>
            )}
          </p>
        </div>
        <Button variant="ghost" onClick={() => void load()}>
          <RotateCw className="h-3.5 w-3.5" />
          Refresh
        </Button>
      </header>

      {error && <Alert tone="error">{error}</Alert>}

      <Card>
        <div className="overflow-x-auto">
          <table className="w-full text-sm">
            <thead>
              <tr className="border-b border-border text-left text-xs text-ink-faint">
                <th className="px-4 py-3 font-medium">Account</th>
                <th className="px-4 py-3 font-medium">Status</th>
                <th className="px-4 py-3 font-medium">Role</th>
                <th className="px-4 py-3 font-medium">Telegram</th>
                <th className="px-4 py-3 font-medium">Last seen</th>
                <th className="px-4 py-3 text-right font-medium">Actions</th>
              </tr>
            </thead>
            <tbody>
              {users.map((user) => {
                const isSelf = user.id === me.id;
                const busy = pendingRow === user.id;
                return (
                  <tr
                    key={user.id}
                    className="border-b border-border-soft last:border-0"
                  >
                    <td className="px-4 py-3">
                      <div className="font-medium text-ink">{user.email}</div>
                      {isSelf && (
                        <div className="text-xs text-ink-faint">You</div>
                      )}
                    </td>
                    <td className="px-4 py-3">
                      <Badge tone={user.status}>{user.status}</Badge>
                    </td>
                    <td className="px-4 py-3">
                      <Badge tone={user.role}>{user.role}</Badge>
                    </td>
                    <td className="px-4 py-3 text-ink-muted">
                      {user.telegram_connected ? "Connected" : "—"}
                    </td>
                    <td className="px-4 py-3 text-ink-muted">
                      {user.last_login_at
                        ? new Date(user.last_login_at).toLocaleDateString()
                        : "Never"}
                    </td>
                    <td className="px-4 py-3">
                      <div className="flex justify-end gap-1.5">
                        {user.status !== "active" && (
                          <Button
                            variant="ghost"
                            busy={busy}
                            onClick={() => setStatus(user.id, "active")}
                            title="Allow this account to sign in"
                          >
                            <Check className="h-3.5 w-3.5" />
                            Approve
                          </Button>
                        )}
                        {user.status === "active" && !isSelf && (
                          <Button
                            variant="ghost"
                            busy={busy}
                            onClick={() => setStatus(user.id, "suspended")}
                            title="Revoke access immediately"
                          >
                            <Ban className="h-3.5 w-3.5" />
                            Suspend
                          </Button>
                        )}
                        {user.role === "user" ? (
                          <Button
                            variant="ghost"
                            busy={busy}
                            onClick={() => setRole(user.id, "admin")}
                            title="Grant administrator access"
                          >
                            <ShieldCheck className="h-3.5 w-3.5" />
                          </Button>
                        ) : (
                          !isSelf && (
                            <Button
                              variant="ghost"
                              busy={busy}
                              onClick={() => setRole(user.id, "user")}
                              title="Revoke administrator access"
                            >
                              <ShieldOff className="h-3.5 w-3.5" />
                            </Button>
                          )
                        )}
                      </div>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </div>
      </Card>

      <section>
        <h2 className="mb-2 text-sm font-semibold text-ink">Recent activity</h2>
        <Card className="divide-y divide-border-soft">
          {audit.length === 0 && (
            <p className="px-4 py-6 text-sm text-ink-faint">Nothing yet.</p>
          )}
          {audit.map((entry) => (
            <div
              key={entry.id}
              className="flex items-baseline justify-between gap-4 px-4 py-2.5 text-sm"
            >
              <span className="font-mono text-xs text-ink-muted">
                {entry.action}
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
