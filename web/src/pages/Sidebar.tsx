import { useState } from "react";
import {
  ChevronLeft,
  Folder as FolderIcon,
  FolderPlus,
  HardDrive,
  Inbox,
  Plus,
  RefreshCcw,
  Tag,
  Unplug,
  X,
} from "lucide-react";
import { api, type Folder, type Group } from "../api";
import { Button } from "../ui";

/// Which folders the list is filtered to.
type Filter = { kind: "all" } | { kind: "unassigned" } | { kind: "group"; id: string };

export function Sidebar({
  folders,
  groups,
  selected,
  phoneHint,
  totalBytes,
  creating,
  syncing,
  onSelect,
  onNewFolder,
  onNewGroup,
  onSync,
  onDisconnect,
  onCollapse,
  onChanged,
}: {
  folders: Folder[];
  groups: Group[];
  selected: string | undefined;
  phoneHint: string | null;
  totalBytes: string;
  creating: boolean;
  syncing: boolean;
  onSelect: (id: string | undefined) => void;
  onNewFolder: () => void;
  onNewGroup: () => void;
  onSync: () => void;
  onDisconnect: () => void;
  onCollapse: () => void;
  onChanged: () => void;
}) {
  const [filter, setFilter] = useState<Filter>({ kind: "all" });
  // Which folder is having its group changed, if any.
  const [tagging, setTagging] = useState<string | null>(null);

  const visible = folders.filter((folder) => {
    if (filter.kind === "all") return true;
    if (filter.kind === "unassigned") return folder.group_id === null;
    return folder.group_id === filter.id;
  });

  async function moveTo(folderId: string, groupId: string | null) {
    setTagging(null);
    await api.groups.assign(folderId, groupId).catch(() => undefined);
    onChanged();
  }

  return (
    <aside className="flex h-full w-64 shrink-0 flex-col border-r border-border bg-surface">
      <div className="flex items-center justify-between gap-2 px-4 py-3.5">
        <div className="flex min-w-0 items-center gap-2.5">
          <div className="flex h-7 w-7 shrink-0 items-center justify-center rounded-[var(--radius-control)] bg-accent">
            <HardDrive className="h-4 w-4 text-white" />
          </div>
          <span className="truncate text-sm font-semibold text-ink">
            Normless Drive
          </span>
        </div>
        <button
          onClick={onCollapse}
          aria-label="Collapse sidebar"
          className="text-ink-faint transition-colors hover:text-ink"
        >
          <ChevronLeft className="h-4 w-4" />
        </button>
      </div>

      {/* Group filter chips, as in the desktop app. Only worth showing
          once a group exists — before that they would all mean the same
          thing. */}
      {groups.length > 0 && (
        <div className="px-3 pb-2">
          <div className="mb-1.5 flex items-center justify-between">
            <span className="text-[11px] font-medium uppercase tracking-wide text-ink-faint">
              Groups
            </span>
            <button
              onClick={onNewGroup}
              aria-label="New group"
              title="New group"
              className="text-ink-faint transition-colors hover:text-ink"
            >
              <Plus className="h-3.5 w-3.5" />
            </button>
          </div>
          <div className="flex flex-wrap gap-1">
            <Chip
              label="All"
              active={filter.kind === "all"}
              onClick={() => setFilter({ kind: "all" })}
            />
            <Chip
              label="Unassigned"
              active={filter.kind === "unassigned"}
              onClick={() => setFilter({ kind: "unassigned" })}
            />
            {groups.map((group) => (
              <Chip
                key={group.id}
                label={group.name}
                colour={group.color_hex}
                active={filter.kind === "group" && filter.id === group.id}
                onClick={() => setFilter({ kind: "group", id: group.id })}
                onRemove={async () => {
                  if (
                    !window.confirm(
                      `Delete the group "${group.name}"? Its folders and files are not touched — they just become ungrouped.`,
                    )
                  ) {
                    return;
                  }
                  await api.groups.remove(group.id).catch(() => undefined);
                  setFilter({ kind: "all" });
                  onChanged();
                }}
              />
            ))}
          </div>
        </div>
      )}

      <nav className="flex-1 overflow-y-auto px-2 pb-2">
        {filter.kind === "all" && (
          <Item
            icon={Inbox}
            label="Saved Messages"
            active={selected === undefined}
            onClick={() => onSelect(undefined)}
          />
        )}

        {visible.length > 0 && (
          <p className="px-2 pb-1 pt-3 text-[11px] font-medium uppercase tracking-wide text-ink-faint">
            Folders
          </p>
        )}

        {visible.map((folder) => (
          <div key={folder.id} className="group/row relative">
            <Item
              icon={FolderIcon}
              label={folder.name}
              active={selected === folder.id}
              colour={
                groups.find((g) => g.id === folder.group_id)?.color_hex
              }
              onClick={() => onSelect(folder.id)}
              onTag={groups.length > 0 ? () => setTagging(folder.id) : undefined}
            />

            {tagging === folder.id && (
              <div className="absolute left-2 right-2 z-10 mt-1 rounded-[var(--radius-control)] border border-border bg-raised p-1 shadow-lg">
                <p className="px-2 py-1 text-[11px] text-ink-faint">Move to</p>
                <button
                  onClick={() => void moveTo(folder.id, null)}
                  className="w-full rounded px-2 py-1.5 text-left text-xs text-ink-muted hover:bg-surface hover:text-ink"
                >
                  No group
                </button>
                {groups.map((group) => (
                  <button
                    key={group.id}
                    onClick={() => void moveTo(folder.id, group.id)}
                    className="flex w-full items-center gap-2 rounded px-2 py-1.5 text-left text-xs text-ink-muted hover:bg-surface hover:text-ink"
                  >
                    <span
                      className="h-2 w-2 shrink-0 rounded-full"
                      style={{ backgroundColor: group.color_hex }}
                    />
                    {group.name}
                  </button>
                ))}
              </div>
            )}
          </div>
        ))}

        {visible.length === 0 && filter.kind !== "all" && (
          <p className="px-2 py-6 text-center text-xs text-ink-faint">
            No folders here yet.
          </p>
        )}
      </nav>

      <div className="border-t border-border p-3">
        <Button
          variant="ghost"
          busy={creating}
          onClick={onNewFolder}
          className="w-full"
        >
          <FolderPlus className="h-3.5 w-3.5" />
          Create folder
        </Button>
        {groups.length === 0 && (
          <Button variant="ghost" onClick={onNewGroup} className="mt-1.5 w-full">
            <Tag className="h-3.5 w-3.5" />
            Create group
          </Button>
        )}
        <Button
          variant="ghost"
          busy={syncing}
          onClick={onSync}
          className="mt-1.5 w-full"
          title="Find folders created in the desktop app"
        >
          <RefreshCcw className="h-3.5 w-3.5" />
          Sync from Telegram
        </Button>

        <div className="mt-3 flex items-center gap-1.5 px-1">
          <span className="h-1.5 w-1.5 rounded-full bg-[var(--color-success)]" />
          <span className="text-xs text-ink-muted">Connected to Telegram</span>
        </div>
        <p className="mt-1 px-1 text-[11px] text-ink-faint">
          {phoneHint ?? ""} · {totalBytes} here
        </p>

        <Button variant="ghost" onClick={onDisconnect} className="mt-3 w-full">
          <Unplug className="h-3.5 w-3.5" />
          Disconnect
        </Button>
      </div>
    </aside>
  );
}

function Chip({
  label,
  active,
  colour,
  onClick,
  onRemove,
}: {
  label: string;
  active: boolean;
  colour?: string;
  onClick: () => void;
  onRemove?: () => void;
}) {
  return (
    <span
      className={`group/chip inline-flex items-center gap-1 rounded-full px-2 py-0.5 text-[11px]
                  font-medium transition-colors ${
                    active
                      ? "bg-accent text-white"
                      : "border border-border text-ink-muted hover:bg-canvas hover:text-ink"
                  }`}
    >
      <button onClick={onClick} className="flex items-center gap-1.5">
        {colour && !active && (
          <span
            className="h-2 w-2 rounded-full"
            style={{ backgroundColor: colour }}
          />
        )}
        {label}
      </button>
      {onRemove && (
        <button
          onClick={onRemove}
          aria-label={`Delete group ${label}`}
          className="opacity-0 transition-opacity group-hover/chip:opacity-100"
        >
          <X className="h-2.5 w-2.5" />
        </button>
      )}
    </span>
  );
}

function Item({
  icon: Icon,
  label,
  active,
  colour,
  onClick,
  onTag,
}: {
  icon: typeof Inbox;
  label: string;
  active: boolean;
  colour?: string;
  onClick: () => void;
  onTag?: () => void;
}) {
  return (
    <div
      className={`flex items-center rounded-[var(--radius-control)] transition-colors ${
        active ? "bg-accent-soft" : "hover:bg-canvas"
      }`}
    >
      <button
        onClick={onClick}
        className={`flex min-w-0 flex-1 items-center gap-2.5 px-2.5 py-2 text-left text-sm ${
          active ? "font-medium text-accent" : "text-ink-muted"
        }`}
      >
        <Icon className="h-4 w-4 shrink-0" />
        <span className="truncate">{label}</span>
        {colour && (
          <span
            className="ml-auto h-2 w-2 shrink-0 rounded-full"
            style={{ backgroundColor: colour }}
          />
        )}
      </button>
      {onTag && (
        <button
          onClick={onTag}
          aria-label={`Move ${label} to a group`}
          title="Move to a group"
          className="mr-1.5 rounded p-1 text-ink-faint opacity-0 transition-opacity hover:text-ink group-hover/row:opacity-100"
        >
          <Tag className="h-3 w-3" />
        </button>
      )}
    </div>
  );
}
