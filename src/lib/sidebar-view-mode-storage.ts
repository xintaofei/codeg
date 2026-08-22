"use client"

const FOLDER_EXPANDED_KEY = "workspace:sidebar-folder-expanded"
const SHOW_COMPLETED_KEY = "workspace:sidebar-show-completed"
const SHOW_WORKTREES_KEY = "workspace:sidebar-show-worktrees"
const SHOW_RECENT_KEY = "workspace:sidebar-show-recent"
const SORT_MODE_KEY = "workspace:sidebar-sort-mode"
const SECTION_ORDER_KEY = "workspace:sidebar-section-order"
const SECTION_COLLAPSED_KEY = "workspace:sidebar-section-collapsed"
const CONVERSATION_EXPANDED_KEY = "workspace:sidebar-conversation-expanded"

export type SidebarSortMode = "created" | "updated"

/** The reorderable top-level sidebar sections. "Pinned" is deliberately absent:
 *  it is a transient override bucket and always stays on top. */
export const SIDEBAR_SECTION_IDS = ["folders", "chats", "recent"] as const

export type SidebarSectionId = (typeof SIDEBAR_SECTION_IDS)[number]

/** Every section that can own a header row — the reorderable ones plus the
 *  always-on-top "Pinned" bucket and the always-on-top "PK" bucket (shown only
 *  when PK arena conversations exist). */
export type SidebarSectionKey = "pinned" | "pk" | SidebarSectionId

/**
 * Vertical order of the reorderable sidebar sections, top to bottom. Always a
 * full permutation of {@link SIDEBAR_SECTION_IDS} — {@link normalizeSectionOrder}
 * guarantees that on the way in, so no section can ever go missing from the
 * list or the reorder menu. The Pinned section (when present) always stays on
 * top regardless.
 */
export type SidebarSectionOrder = readonly SidebarSectionId[]

/** Folders, then Chat, then Recent — the historical layout with the (newest)
 *  Recent section appended at the bottom. */
export const DEFAULT_SECTION_ORDER: SidebarSectionOrder = SIDEBAR_SECTION_IDS

/** Collapsed state of the top-level sidebar sections. Absent key = expanded
 *  (the default), so a fresh user sees every section open. */
export interface SidebarSectionCollapsed {
  pinned?: boolean
  pk?: boolean
  folders?: boolean
  chats?: boolean
  recent?: boolean
}

function isSectionId(value: unknown): value is SidebarSectionId {
  return (SIDEBAR_SECTION_IDS as readonly unknown[]).includes(value)
}

/**
 * Coerce an arbitrary stored value into a full, duplicate-free permutation of
 * {@link SIDEBAR_SECTION_IDS}.
 *
 * Accepts three shapes:
 * - an array of section ids (the current format) — unknown entries and repeats
 *   are dropped, and any section the array omits is appended in default order.
 *   That last step is what makes adding a NEW section forward-compatible: an
 *   older store simply gets it at the bottom instead of losing it.
 * - the legacy pre-Recent strings `"folders-first"` / `"chats-first"`, so a
 *   user's existing top/bottom choice survives the upgrade.
 * - anything else (corrupt / absent) → {@link DEFAULT_SECTION_ORDER}.
 */
export function normalizeSectionOrder(value: unknown): SidebarSectionOrder {
  if (value === "chats-first") return ["chats", "folders", "recent"]
  if (value === "folders-first") return DEFAULT_SECTION_ORDER
  if (!Array.isArray(value)) return DEFAULT_SECTION_ORDER
  const seen = new Set<SidebarSectionId>()
  const out: SidebarSectionId[] = []
  for (const entry of value) {
    if (!isSectionId(entry) || seen.has(entry)) continue
    seen.add(entry)
    out.push(entry)
  }
  for (const id of SIDEBAR_SECTION_IDS) {
    if (!seen.has(id)) out.push(id)
  }
  return out
}

/**
 * Move `id` by `delta` slots (negative = towards the top), returning a new
 * order. A move that would fall off either end — or that names an id not in the
 * order — returns the SAME array reference, so a no-op never re-renders the
 * sidebar or rewrites localStorage.
 */
export function moveSectionInOrder(
  order: SidebarSectionOrder,
  id: SidebarSectionId,
  delta: number
): SidebarSectionOrder {
  const from = order.indexOf(id)
  if (from < 0) return order
  const to = from + delta
  if (to < 0 || to >= order.length || to === from) return order
  const next = order.slice()
  next.splice(from, 1)
  next.splice(to, 0, id)
  return next
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
 *  OFF so finished/imported sessions stay out of the way; only an
 *  explicitly-stored "true" (the user turned it on) reveals them. */
export function loadShowCompleted(): boolean {
  if (typeof window === "undefined") return false
  try {
    const raw = localStorage.getItem(SHOW_COMPLETED_KEY)
    if (raw === "true") return true
    if (raw === "false") return false
  } catch {
    /* ignore */
  }
  return false
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

/** Whether the flat "Recent" section — every conversation, folder-bound or
 *  chat, newest first — is shown. Defaults to ON; only an explicitly-stored
 *  "false" (the user unchecked it) hides the section. Its POSITION is a
 *  separate preference ({@link loadSectionOrder}), so hiding and re-showing it
 *  keeps the slot the user chose. */
export function loadShowRecent(): boolean {
  if (typeof window === "undefined") return true
  try {
    const raw = localStorage.getItem(SHOW_RECENT_KEY)
    if (raw === "false") return false
    if (raw === "true") return true
  } catch {
    /* ignore */
  }
  return true
}

export function saveShowRecent(value: boolean): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(SHOW_RECENT_KEY, String(value))
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
  if (typeof window === "undefined") return DEFAULT_SECTION_ORDER
  try {
    const raw = localStorage.getItem(SECTION_ORDER_KEY)
    if (!raw) return DEFAULT_SECTION_ORDER
    // Current format is a JSON array; the legacy pre-Recent format was a bare
    // `folders-first` / `chats-first` string, which is not valid JSON — fall
    // back to the raw string so `normalizeSectionOrder` can migrate it.
    let parsed: unknown
    try {
      parsed = JSON.parse(raw)
    } catch {
      parsed = raw
    }
    return normalizeSectionOrder(parsed)
  } catch {
    /* ignore */
  }
  return DEFAULT_SECTION_ORDER
}

export function saveSectionOrder(value: SidebarSectionOrder): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(SECTION_ORDER_KEY, JSON.stringify(value))
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
    if (typeof obj.recent === "boolean") result.recent = obj.recent
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
