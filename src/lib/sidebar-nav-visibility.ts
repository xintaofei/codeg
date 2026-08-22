/** The sidebar's fixed nav rows that can be hidden from Settings →
 *  Appearance. "New chat" is deliberately absent: it is the primary entry
 *  point into the workspace and must never disappear. Ids match the
 *  workbench route ids the rows navigate to. */
export const HIDEABLE_SIDEBAR_NAV_IDS = [
  "automations",
  "tasks",
  "forge",
] as const

export type HideableSidebarNavId = (typeof HIDEABLE_SIDEBAR_NAV_IDS)[number]

/** Visibility of each hideable fixed nav row. Always a complete record —
 *  {@link parseSidebarNavVisibility} guarantees that on the way in, so
 *  consumers never need per-key fallbacks. */
export type SidebarNavVisibility = Record<HideableSidebarNavId, boolean>

export const DEFAULT_SIDEBAR_NAV_VISIBILITY: SidebarNavVisibility = {
  automations: true,
  tasks: true,
  forge: true,
}

/**
 * Coerce the stored JSON into a complete visibility record. Absent or corrupt
 * input falls back to every row visible (the historical layout); unknown keys
 * are dropped and missing ones default to visible. That last step is what
 * makes adding a NEW hideable row forward-compatible: an older stored record
 * simply shows it instead of losing it.
 */
export function parseSidebarNavVisibility(
  raw: string | null
): SidebarNavVisibility {
  if (!raw) return DEFAULT_SIDEBAR_NAV_VISIBILITY
  let parsed: unknown
  try {
    parsed = JSON.parse(raw)
  } catch {
    return DEFAULT_SIDEBAR_NAV_VISIBILITY
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return DEFAULT_SIDEBAR_NAV_VISIBILITY
  }
  const obj = parsed as Record<string, unknown>
  const out = { ...DEFAULT_SIDEBAR_NAV_VISIBILITY }
  for (const id of HIDEABLE_SIDEBAR_NAV_IDS) {
    const value = obj[id]
    if (typeof value === "boolean") out[id] = value
  }
  return out
}
