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

export interface SharePermissions {
  can_upload: boolean;
  can_download: boolean;
  can_update: boolean;
  can_delete: boolean;
}

export interface Share {
  id: string;
  token: string;
  url: string;
  label: string | null;
  folder_id: string | null;
  permissions: SharePermissions;
  has_password: boolean;
  expires_at: string | null;
  usage_limit: number | null;
  usage_count: number;
  allow_offline: boolean;
  file_count: number;
  created_at: string;
}

export interface Folder {
  id: string;
  channel_id: number;
  name: string;
  group_id: string | null;
  created_at: string;
}

export interface Group {
  id: string;
  name: string;
  color_hex: string;
  display_order: number;
}

export interface Permissions {
  can_upload: boolean;
  can_download: boolean;
  can_delete: boolean;
  can_create_folders: boolean;
  can_share: boolean;
  /// Bytes. Zero means no cap.
  storage_quota_bytes: number;
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

  /// The cookie is still valid but the server cannot decrypt this user's
  /// Telegram session — its key lives only in memory and is lost on
  /// restart. Signing in again is the one thing that fixes it, so the UI
  /// says so rather than showing a generic refusal.
  get isLocked() {
    return this.code === "SESSION_LOCKED";
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

  unlock: (password: string) =>
    request<{ ok: true }>("/api/auth/unlock", {
      method: "POST",
      body: json({ password }),
    }),

  logout: () => request<{ ok: true }>("/api/auth/logout", { method: "POST" }),

  me: () => request<User>("/api/auth/me"),

  shares: {
    list: () =>
      request<{ shares: Share[]; offline_available: boolean }>("/api/shares"),

    create: (input: {
      label?: string;
      folder_id?: string;
      permissions: SharePermissions;
      message_ids?: number[];
      password?: string;
      expiry_hours?: number;
      usage_limit?: number;
      allow_offline?: boolean;
    }) => request<Share>("/api/shares", { method: "POST", body: json(input) }),

    revoke: (id: string) =>
      request<{ ok: true }>(`/api/shares/${id}`, { method: "DELETE" }),

    setOffline: (enabled: boolean, password?: string) =>
      request<{ ok: true; enabled: boolean }>("/api/shares/offline", {
        method: "POST",
        body: json({ enabled, password }),
      }),
  },

  /// The unauthenticated side, used only by the public share page.
  publicShare: {
    info: (token: string) =>
      request<{
        label: string | null;
        permissions: SharePermissions;
        needs_password: boolean;
        unlocked: boolean;
        available: boolean;
      }>(`/api/s/${token}`),

    verify: (token: string, password: string) =>
      request<{ ok: true }>(`/api/s/${token}/verify`, {
        method: "POST",
        body: json({ password }),
      }),

    files: (token: string) =>
      request<{ files: FileRow[] }>(`/api/s/${token}/files`),

    downloadUrl: (token: string, id: number) =>
      `/api/s/${token}/files/${id}/download`,
  },

  groups: {
    list: () => request<{ groups: Group[] }>("/api/groups"),
    create: (name: string, color_hex?: string) =>
      request<Group>("/api/groups", {
        method: "POST",
        body: json({ name, color_hex }),
      }),
    remove: (id: string) =>
      request<{ ok: true }>(`/api/groups/${id}`, { method: "DELETE" }),
    assign: (folder_id: string, group_id: string | null) =>
      request<{ ok: true }>("/api/groups/assign", {
        method: "POST",
        body: json({ folder_id, group_id }),
      }),
  },

  folders: {
    list: () => request<{ folders: Folder[] }>("/api/folders"),
    sync: () =>
      request<{ seen: number; added: number }>("/api/folders/sync", {
        method: "POST",
      }),

    create: (name: string) =>
      request<Folder>("/api/folders", {
        method: "POST",
        body: json({ name }),
      }),
  },

  files: {
    list: (folder?: string) =>
      request<{ files: FileRow[] }>(
        folder ? `/api/files?folder=${folder}` : "/api/files",
      ),

    remote: (url: string, folder?: string, name?: string) =>
      request<{ ok: true; name: string; size: number }>("/api/files/remote", {
        method: "POST",
        body: json({ url, folder, name }),
      }),

    remove: (id: number, folder?: string) =>
      request<{ ok: true }>(
        folder ? `/api/files/${id}?folder=${folder}` : `/api/files/${id}`,
        { method: "DELETE" },
      ),

    downloadUrl: (id: number, folder?: string) =>
      folder
        ? `/api/files/${id}/download?folder=${folder}`
        : `/api/files/${id}/download`,

    upload: async (file: File, folder?: string) => {
      const form = new FormData();
      form.append("file", file);
      // No Content-Type header: the browser must set it so the multipart
      // boundary is included, and overriding it breaks the upload.
      // The size travels in the query because Telegram needs the length
      // before the transfer starts, and the browser is the only party that
      // already knows it. Without this the server would have to buffer the
      // whole file just to measure it.
      const params = new URLSearchParams({ size: String(file.size) });
      if (folder) params.set("folder", folder);

      const response = await fetch(`/api/files?${params}`, {
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

    qrStart: (api_id: number, api_hash: string) =>
      request<{ url: string | null; expires: number | null; connected: boolean }>(
        "/api/telegram/qr/start",
        { method: "POST", body: json({ api_id, api_hash }) },
      ),

    qrPoll: () =>
      request<{ url: string | null; expires: number | null; connected: boolean }>(
        "/api/telegram/qr/poll",
        { method: "POST" },
      ),

    start: (api_id: number, api_hash: string, phone: string) =>
      request<{ sent: boolean; phone_hint: string; delivery: string; numeric: boolean; next: string | null }>("/api/telegram/start", {
        method: "POST",
        body: json({ api_id, api_hash, phone }),
      }),

    resend: () =>
      request<{ delivery: string; numeric: boolean; next: string | null }>(
        "/api/telegram/resend",
        { method: "POST" },
      ),

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

    permissions: (id: string) =>
      request<{ permissions: Permissions; used_bytes: number }>(
        `/api/admin/users/${id}/permissions`,
      ),

    setPermissions: (id: string, permissions: Permissions) =>
      request<{ ok: true }>(`/api/admin/users/${id}/permissions`, {
        method: "POST",
        body: json(permissions),
      }),

    audit: (limit = 100) =>
      request<{ entries: AuditEntry[] }>(`/api/admin/audit?limit=${limit}`),
  },
};
