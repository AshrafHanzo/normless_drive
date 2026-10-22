// Thin wrapper over fetch for the Normless Drive API.
//
// Every call sends cookies: the session lives in an HttpOnly cookie, which
// JavaScript cannot read, so there is no token to attach by hand and nothing
// for an XSS bug to steal.

export type Role = "admin" | "user";
export type Status = "pending" | "active" | "suspended";

export interface User {
  id: string;
  email: string;
  role: Role;
  status: Status;
}

export interface AdminUserRow extends User {
  created_at: string;
  last_login_at: string | null;
  telegram_connected: boolean;
}

export interface FileRow {
  id: number;
  name: string;
  size: number;
  mime_type: string | null;
  /// Unix seconds, as Telegram reports it.
  date: number;
}

export interface AuditEntry {
  id: number;
  user_id: string | null;
  actor_id: string | null;
  action: string;
  detail: unknown;
  created_at: string;
}

/// An error carrying the server's own message, so the UI can show what the
/// API actually said rather than a generic "request failed".
export class ApiError extends Error {
  constructor(
    public status: number,
    public code: string,
    message: string,
  ) {
    super(message);
    this.name = "ApiError";
  }

  /// True when signing in would plausibly fix this.
  get isAuthProblem() {
    return this.status === 401;
  }
}

async function request<T>(
  path: string,
  init: RequestInit = {},
): Promise<T> {
  let response: Response;
  try {
    response = await fetch(path, {
      ...init,
      credentials: "same-origin",
      headers: {
        "Content-Type": "application/json",
        ...(init.headers ?? {}),
      },
    });
  } catch {
    // A rejected fetch is a transport failure, not an HTTP status — the
    // server is unreachable, DNS failed, or the user is offline.
    throw new ApiError(0, "NETWORK", "Could not reach the server.");
  }

  if (response.status === 204) {
    return undefined as T;
  }

  const text = await response.text();
  let body: unknown = null;
  if (text) {
    try {
      body = JSON.parse(text);
    } catch {
      // A non-JSON body from a reverse proxy (502 pages, upload size
      // rejections) would otherwise surface as "Unexpected token <".
      if (!response.ok) {
        throw new ApiError(
          response.status,
          "UNEXPECTED",
          `Server returned ${response.status}.`,
        );
      }
    }
  }

  if (!response.ok) {
    const detail = (body as { error?: { code?: string; message?: string } })
      ?.error;
    throw new ApiError(
      response.status,
      detail?.code ?? "UNKNOWN",
      detail?.message ?? `Request failed with ${response.status}.`,
    );
  }

  return body as T;
}

const json = (value: unknown) => JSON.stringify(value);

export const api = {
  register: (email: string, password: string) =>
    request<{ user: User; message: string }>("/api/auth/register", {
      method: "POST",
      body: json({ email, password }),
    }),

  login: (email: string, password: string) =>
    request<User>("/api/auth/login", {
      method: "POST",
      body: json({ email, password }),
    }),

  logout: () => request<{ ok: true }>("/api/auth/logout", { method: "POST" }),

  me: () => request<User>("/api/auth/me"),

  files: {
    list: () => request<{ files: FileRow[] }>("/api/files"),

    upload: async (file: File) => {
      const form = new FormData();
      form.append("file", file);
      // No Content-Type header: the browser must set it so the multipart
      // boundary is included, and overriding it breaks the upload.
      const response = await fetch("/api/files", {
        method: "POST",
        credentials: "same-origin",
        body: form,
      });
      if (!response.ok) {
        const body = await response.json().catch(() => null);
        const detail = (body as { error?: { code?: string; message?: string } })
          ?.error;
        throw new ApiError(
          response.status,
          detail?.code ?? "UNKNOWN",
          detail?.message ?? `Upload failed with ${response.status}.`,
        );
      }
      return response.json() as Promise<{ ok: true; name: string; size: number }>;
    },
  },

  telegram: {
    status: () =>
      request<{
        connected: boolean;
        phone_hint: string | null;
        connected_at: string | null;
        unlocked: boolean;
      }>("/api/telegram/status"),

    start: (api_id: number, api_hash: string, phone: string) =>
      request<{ sent: boolean; phone_hint: string }>("/api/telegram/start", {
        method: "POST",
        body: json({ api_id, api_hash, phone }),
      }),

    code: (code: string) =>
      request<{ connected: boolean; needs_password: boolean }>(
        "/api/telegram/code",
        { method: "POST", body: json({ code }) },
      ),

    password: (password: string) =>
      request<{ connected: boolean; needs_password: boolean }>(
        "/api/telegram/password",
        { method: "POST", body: json({ password }) },
      ),

    disconnect: () =>
      request<{ ok: true; was_connected: boolean }>(
        "/api/telegram/disconnect",
        { method: "POST" },
      ),
  },

  admin: {
    users: () => request<{ users: AdminUserRow[] }>("/api/admin/users"),

    setStatus: (id: string, status: Status) =>
      request<{ ok: true; status: Status }>(`/api/admin/users/${id}/status`, {
        method: "POST",
        body: json({ status }),
      }),

    setRole: (id: string, role: Role) =>
      request<{ ok: true; role: Role }>(`/api/admin/users/${id}/role`, {
        method: "POST",
        body: json({ role }),
      }),

    audit: (limit = 100) =>
      request<{ entries: AuditEntry[] }>(`/api/admin/audit?limit=${limit}`),
  },
};
