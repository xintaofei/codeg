import type { ForgeTaskLink } from "@/lib/types"

/** Non-terminal statuses — mirror of the backend's ACTIVE_STATUSES. */
const ACTIVE_STATUSES = new Set([
  "todo",
  "queued",
  "preparing",
  "running",
  "awaiting_input",
  "review",
  "merging",
])

export type ForgeChipState = "none" | "active" | "terminal"

/**
 * The workbench row's three-state action, derived from the reverse lookup:
 * no task → offer Start; an ACTIVE task → live status chip (click-through);
 * a terminal task → done/canceled chip with a re-trigger affordance.
 */
export function chipStateForLink(
  link: ForgeTaskLink | null | undefined
): ForgeChipState {
  if (link == null) return "none"
  return ACTIVE_STATUSES.has(link.status) ? "active" : "terminal"
}
