"use client"

const FOLDER_EXPANDED_KEY = "workspace:sidebar-folder-expanded"
const SHOW_COMPLETED_KEY = "workspace:sidebar-show-completed"
const SHOW_WORKTREES_KEY = "workspace:sidebar-show-worktrees"
const SORT_MODE_KEY = "workspace:sidebar-sort-mode"
const SECTION_ORDER_KEY = "workspace:sidebar-section-order"
const SECTION_COLLAPSED_KEY = "workspace:sidebar-section-collapsed"
const CONVERSATION_EXPANDED_KEY = "workspace:sidebar-conversation-expanded"

export type SidebarSortMode = "created" | "updated"

/** Vertical order of the Folders and Chat sections in the sidebar list. The
 *  Pinned section (when present) always stays on top and is not reordered.
 *  Default `folders-first` preserves the historical layout. */
export type SidebarSectionOrder = "folders-first" | "chats-first"

/** Collapsed state of the two top-level sidebar sections. Absent key = expanded
 *  (the default), so a fresh user sees both sections open. */
export interface SidebarSectionCollapsed {
  pinned?: boolean
  folders?: boolean
  chats?: boolean
}

export function loadFolderExpanded(): Record<number, boolean> {
  if (typeof window === "undefined") return {}
  try {
    const raw = localStorage.getItem(FOLDER_EXPANDED_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as unknown
    if (!parsed || typeof parsed !== "object") return {}
    const result: Record<number, boolean> = {}
    for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
      const id = Number(k)
      if (!Number.isNaN(id) && typeof v === "boolean") {
        result[id] = v
      }
    }
    return result
  } catch {
    return {}
  }
}

export function saveFolderExpanded(state: Record<number, boolean>): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(FOLDER_EXPANDED_KEY, JSON.stringify(state))
  } catch {
    /* ignore */
  }
}

/** Ids of conversations whose delegation sub-session subtree is expanded. Stored
 *  as a flat array (not a Record) because the default is COLLAPSED — only the
 *  expanded ids are persisted, keeping storage bounded by what the user actually
 *  opened (unlike folders, which default to expanded). */
export function loadConversationExpanded(): number[] {
  if (typeof window === "undefined") return []
  try {
    const raw = localStorage.getItem(CONVERSATION_EXPANDED_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as unknown
    if (!Array.isArray(parsed)) return []
    const result: number[] = []
    for (const v of parsed) {
      if (typeof v === "number" && Number.isFinite(v)) result.push(v)
    }
    return result
  } catch {
    return []
  }
}

export function saveConversationExpanded(ids: number[]): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(CONVERSATION_EXPANDED_KEY, JSON.stringify(ids))
  } catch {
    /* ignore */
  }
}

/** Whether completed conversations are shown in the sidebar list. Defaults to
 *  ON; only an explicitly-stored "false" (the user unchecked it) hides them. */
export function loadShowCompleted(): boolean {
  if (typeof window === "undefined") return true
  try {
    const raw = localStorage.getItem(SHOW_COMPLETED_KEY)
    if (raw === "false") return false
    if (raw === "true") return true
  } catch {
    /* ignore */
  }
  return true
}

export function saveShowCompleted(value: boolean): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(SHOW_COMPLETED_KEY, String(value))
  } catch {
    /* ignore */
  }
}

/** Whether the sidebar splits each repo's worktree child folders into their own
 *  indented sub-groups (instead of merging them flat into the parent group).
 *  Defaults to ON; only an explicitly-stored "false" (the user unchecked it)
 *  falls back to the flattened layout. */
export function loadShowWorktrees(): boolean {
  if (typeof window === "undefined") return true
  try {
    const raw = localStorage.getItem(SHOW_WORKTREES_KEY)
    if (raw === "false") return false
    if (raw === "true") return true
  } catch {
    /* ignore */
  }
  return true
}

export function saveShowWorktrees(value: boolean): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(SHOW_WORKTREES_KEY, String(value))
  } catch {
    /* ignore */
  }
}

export function loadSortMode(): SidebarSortMode {
  if (typeof window === "undefined") return "created"
  try {
    const raw = localStorage.getItem(SORT_MODE_KEY)
    if (raw === "updated" || raw === "created") return raw
  } catch {
    /* ignore */
  }
  return "created"
}

export function saveSortMode(value: SidebarSortMode): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(SORT_MODE_KEY, value)
  } catch {
    /* ignore */
  }
}

export function loadSectionOrder(): SidebarSectionOrder {
  if (typeof window === "undefined") return "folders-first"
  try {
    const raw = localStorage.getItem(SECTION_ORDER_KEY)
    if (raw === "folders-first" || raw === "chats-first") return raw
  } catch {
    /* ignore */
  }
  return "folders-first"
}

export function saveSectionOrder(value: SidebarSectionOrder): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(SECTION_ORDER_KEY, value)
  } catch {
    /* ignore */
  }
}

export function loadSectionCollapsed(): SidebarSectionCollapsed {
  if (typeof window === "undefined") return {}
  try {
    const raw = localStorage.getItem(SECTION_COLLAPSED_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as unknown
    if (!parsed || typeof parsed !== "object") return {}
    const obj = parsed as Record<string, unknown>
    const result: SidebarSectionCollapsed = {}
    if (typeof obj.pinned === "boolean") result.pinned = obj.pinned
    if (typeof obj.folders === "boolean") result.folders = obj.folders
    if (typeof obj.chats === "boolean") result.chats = obj.chats
    return result
  } catch {
    return {}
  }
}

export function saveSectionCollapsed(state: SidebarSectionCollapsed): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(SECTION_COLLAPSED_KEY, JSON.stringify(state))
  } catch {
    /* ignore */
  }
}
