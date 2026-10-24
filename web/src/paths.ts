// Turning a flat list of slash-separated paths into something you can walk
// a level at a time.
//
// Both the folder-upload review and the archive viewer hold exactly this:
// every file's full path and its size, with no directory entries worth
// trusting. Directories are therefore derived rather than read — a folder
// exists because something inside it does.

export interface Item {
  path: string;
  size: number;
}

export interface Node {
  /// Full path, so a directory can be navigated into directly.
  path: string;
  name: string;
  directory: boolean;
  /// For a directory, everything beneath it added up.
  size: number;
  /// Files beneath a directory, at any depth.
  count: number;
}

/// Everything directly inside `cwd`, with deeper files folded into the
/// directory that contains them.
///
/// `cwd` is "" for the root, otherwise a path with no trailing slash.
export function childrenOf<T extends Item>(items: T[], cwd: string): Node[] {
  const prefix = cwd ? `${cwd}/` : "";
  const directories = new Map<string, Node>();
  const files: Node[] = [];

  for (const item of items) {
    if (prefix && !item.path.startsWith(prefix)) continue;
    const rest = item.path.slice(prefix.length);
    if (!rest) continue;

    const cut = rest.indexOf("/");
    if (cut === -1) {
      files.push({
        path: item.path,
        name: rest,
        directory: false,
        size: item.size,
        count: 1,
      });
      continue;
    }

    const name = rest.slice(0, cut);
    const path = `${prefix}${name}`;
    const existing = directories.get(path);
    if (existing) {
      existing.size += item.size;
      existing.count += 1;
    } else {
      directories.set(path, {
        path,
        name,
        directory: true,
        size: item.size,
        count: 1,
      });
    }
  }

  const collate = (a: Node, b: Node) =>
    a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: "base" });

  // Directories first, the way every file manager does it.
  return [...[...directories.values()].sort(collate), ...files.sort(collate)];
}

/// The path segments of `cwd`, paired with the path that reaches each one —
/// what a breadcrumb needs.
export function trail(cwd: string): { name: string; path: string }[] {
  if (!cwd) return [];
  const parts = cwd.split("/").filter(Boolean);
  return parts.map((name, index) => ({
    name,
    path: parts.slice(0, index + 1).join("/"),
  }));
}

export function parentOf(cwd: string): string {
  const cut = cwd.lastIndexOf("/");
  return cut === -1 ? "" : cwd.slice(0, cut);
}

export function formatSize(bytes: number) {
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  return `${value.toFixed(unit === 0 ? 0 : 1)} ${units[unit]}`;
}
