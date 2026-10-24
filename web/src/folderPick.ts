// Choosing a folder, with the least alarming prompt the browser will give.
//
// An `<input webkitdirectory>` makes Chrome ask "Upload 57 files to this
// site?" the moment a folder is chosen — a count, the word *upload*, and a
// warning, before anyone has decided anything. The File System Access API
// asks to *view* the folder instead, once, with no count, and hands back a
// handle we can walk ourselves. That is the same permission in substance and
// a far calmer question, so it is preferred where it exists (Chrome, Edge,
// Opera) and the input is kept for Firefox and Safari, which have no
// equivalent.
//
// Either way the real confirmation is ours: nothing is sent until the review
// dialog is accepted.

export interface PickedFile {
  file: File;
  /// Path relative to the picked folder — "2026/report.pdf", never the
  /// folder's own name.
  path: string;
}

export interface Picked {
  name: string;
  files: PickedFile[];
}

/// Enough to notice a mistaken pick of a home directory before the browser
/// spends ten minutes enumerating it.
const MAX_FILES = 20000;

// Declared here rather than taken from lib.dom: TypeScript ships only part
// of this API, and the async iteration used below is the part it omits.
interface FileEntry {
  kind: "file";
  name: string;
  getFile(): Promise<File>;
}
interface DirectoryEntry {
  kind: "directory";
  name: string;
  values(): AsyncIterableIterator<FileEntry | DirectoryEntry>;
}
type Picker = (options?: { id?: string; mode?: "read" | "readwrite" }) => Promise<DirectoryEntry>;

function picker(): Picker | null {
  const candidate = (window as unknown as { showDirectoryPicker?: Picker })
    .showDirectoryPicker;
  return typeof candidate === "function" ? candidate : null;
}

export function supportsDirectoryHandle(): boolean {
  return picker() !== null;
}

export class TooManyFiles extends Error {
  constructor() {
    super(
      `That folder holds more than ${MAX_FILES.toLocaleString()} files. Pick a smaller one, or upload it in parts.`,
    );
  }
}

/// Opens the browser's folder picker and reads the tree.
///
/// Resolves to null when the person cancels, which is not an error and
/// should not look like one.
export async function pickDirectory(
  onProgress?: (count: number) => void,
): Promise<Picked | null> {
  const open = picker();
  if (!open) return null;

  let root: DirectoryEntry;
  try {
    root = await open({ id: "normless-drive-folder", mode: "read" });
  } catch (caught) {
    // AbortError is the cancel button. Anything else — a blocked permission,
    // an unsupported context — is worth surfacing.
    if (caught instanceof DOMException && caught.name === "AbortError") return null;
    throw caught;
  }

  const files: PickedFile[] = [];
  await walk(root, "", files, onProgress);
  return { name: root.name, files };
}

async function walk(
  directory: DirectoryEntry,
  prefix: string,
  into: PickedFile[],
  onProgress?: (count: number) => void,
) {
  for await (const entry of directory.values()) {
    const path = prefix ? `${prefix}/${entry.name}` : entry.name;
    if (entry.kind === "file") {
      into.push({ file: await entry.getFile(), path });
      if (into.length > MAX_FILES) throw new TooManyFiles();
      // Reporting every file would re-render thousands of times for no
      // extra information.
      if (into.length % 50 === 0) onProgress?.(into.length);
    } else {
      await walk(entry, path, into, onProgress);
    }
  }
}

/// The same shape, from an `<input webkitdirectory>` — the fallback path.
///
/// `webkitRelativePath` is "Photos/2026/a.jpg" for a pick of Photos; the
/// leading segment is the folder's own name and is stripped, or everything
/// would end up nested one level deeper than was chosen.
export function fromInput(list: FileList | null): Picked | null {
  const files = Array.from(list ?? []);
  if (files.length === 0) return null;

  const first = files[0].webkitRelativePath ?? "";
  const name = first.split("/")[0] || "folder";

  return {
    name,
    files: files.map((file) => {
      const full = file.webkitRelativePath;
      if (!full) return { file, path: file.name };
      const cut = full.indexOf("/");
      return { file, path: cut === -1 ? full : full.slice(cut + 1) };
    }),
  };
}
