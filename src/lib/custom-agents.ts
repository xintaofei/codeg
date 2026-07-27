/**
 * Display metadata for **custom (user-registered) ACP agents**.
 *
 * Built-in agents have compile-time labels and colors (`AGENT_LABELS` /
 * `AGENT_COLORS` in `types.ts`). Custom agents are only known at runtime, so
 * those records can no longer be indexed directly — every call site goes
 * through {@link getAgentLabel} / {@link getAgentColor} instead.
 *
 * The store is a plain module-level map rather than React state on purpose:
 * labels are read from ~70 render paths, and threading a context through all
 * of them would be far more invasive than the problem warrants. It is
 * populated once from the agent list the settings/connection layer already
 * fetches, and until then {@link getAgentLabel} falls back to a humanized form
 * of the registry id — which for most registry entries (`goose`, `qwen-code`,
 * `amp-acp`) is already the right answer, so there is no flash of a broken
 * label.
 */

import {
  AGENT_COLORS,
  AGENT_LABELS,
  customAgentId,
  isCustomAgentType,
  type AgentType,
  type BuiltinAgentType,
} from "@/lib/types"

const customAgentNames = new Map<string, string>()
const customAgentIcons = new Map<string, string>()

// Version counter + listeners so a component can re-render when the map is
// hydrated. Labels tolerate arriving late (the humanized-id fallback is usually
// already right), but an icon has no such fallback — without this, the first
// paint's initial glyph would stick until something else re-rendered the row.
let displayVersion = 0
const displayListeners = new Set<() => void>()

/** Subscribe to changes published by {@link setCustomAgentDisplay}. */
export function subscribeCustomAgentDisplay(listener: () => void): () => void {
  displayListeners.add(listener)
  return () => {
    displayListeners.delete(listener)
  }
}

/** Monotonic snapshot for `useSyncExternalStore`. */
export function getCustomAgentDisplayVersion(): number {
  return displayVersion
}

/**
 * Publish display metadata for the registered custom agents. Safe to call
 * repeatedly; later calls replace the whole set so a deleted agent stops
 * resolving.
 */
export function setCustomAgentDisplay(
  entries: { agentType: AgentType; name: string; iconUrl?: string | null }[]
): void {
  const names = new Map<string, string>()
  const icons = new Map<string, string>()
  for (const entry of entries) {
    const id = customAgentId(entry.agentType)
    if (!id) continue
    if (entry.name.trim()) {
      names.set(id, entry.name.trim())
    }
    const icon = entry.iconUrl?.trim()
    if (icon) {
      icons.set(id, icon)
    }
  }

  // The agent list is refetched on a number of unrelated events; only wake
  // subscribers when what they render actually moved.
  const changed =
    !sameEntries(customAgentNames, names) ||
    !sameEntries(customAgentIcons, icons)
  customAgentNames.clear()
  customAgentIcons.clear()
  for (const [id, name] of names) customAgentNames.set(id, name)
  for (const [id, icon] of icons) customAgentIcons.set(id, icon)
  if (!changed) return
  displayVersion += 1
  for (const listener of displayListeners) listener()
}

function sameEntries(a: Map<string, string>, b: Map<string, string>): boolean {
  if (a.size !== b.size) return false
  for (const [key, value] of a) {
    if (b.get(key) !== value) return false
  }
  return true
}

/**
 * The agent's own mark, when it has one: a `data:` URL inlined at add time, or
 * a plain URL for a definition stored before inlining existed. `null` means
 * "fall back to the initial glyph" — the registry does not require an icon, and
 * a manually added agent need not supply one.
 */
export function getAgentIconUrl(agentType: AgentType): string | null {
  const id = customAgentId(agentType)
  if (!id) return null
  return customAgentIcons.get(id) ?? null
}

/** Turn `github-copilot-cli` into `Github Copilot Cli`. */
function humanizeId(id: string): string {
  return id
    .split(/[-_.]/)
    .filter(Boolean)
    .map((part) => part.charAt(0).toUpperCase() + part.slice(1))
    .join(" ")
}

/**
 * Human-readable name for any agent type. Built-ins use their pinned label;
 * custom agents use their registered name, falling back to a humanized id.
 */
export function getAgentLabel(agentType: AgentType): string {
  const builtin = AGENT_LABELS[agentType as BuiltinAgentType]
  if (builtin) return builtin
  const id = customAgentId(agentType)
  if (!id) return agentType
  return customAgentNames.get(id) ?? humanizeId(id)
}

/**
 * Palette for custom agents. They have no brand color, so pick deterministically
 * from the id — the same agent always gets the same swatch, and neighbouring
 * agents in a list are unlikely to collide.
 */
const CUSTOM_AGENT_COLORS = [
  "bg-sky-600",
  "bg-violet-600",
  "bg-teal-600",
  "bg-rose-600",
  "bg-amber-600",
  "bg-indigo-600",
  "bg-lime-600",
  "bg-fuchsia-600",
] as const

function hashId(id: string): number {
  let hash = 0
  for (let i = 0; i < id.length; i++) {
    hash = (hash * 31 + id.charCodeAt(i)) | 0
  }
  return Math.abs(hash)
}

/** Tailwind background class for an agent's dot/swatch. */
export function getAgentColor(agentType: AgentType): string {
  const builtin = AGENT_COLORS[agentType as BuiltinAgentType]
  if (builtin) return builtin
  const id = customAgentId(agentType)
  if (!id) return "bg-muted-foreground"
  return CUSTOM_AGENT_COLORS[hashId(id) % CUSTOM_AGENT_COLORS.length]
}

/**
 * Single uppercase letter used as a custom agent's icon glyph, for agents with
 * no icon of their own (see {@link getAgentIconUrl}).
 */
export function getAgentInitial(agentType: AgentType): string {
  return getAgentLabel(agentType).trim().charAt(0).toUpperCase() || "?"
}

export { isCustomAgentType, customAgentId }
