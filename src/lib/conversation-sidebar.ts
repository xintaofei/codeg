import type { DbConversationSummary } from "@/lib/types"

/**
 * Whether a conversation belongs in the workspace sidebar as a top-level row
 * (folder group / Chat / Pinned).
 *
 * Delegation children nest under their parent when "Show delegated sub-sessions"
 * is on — they must never appear as peer roots under a folder. Match any of:
 *   - `parent_id` set
 *   - `kind === "delegate"`
 *   - `delegation_call_id` set (broker task id stamped at spawn)
 * Loop rows belong to the loops workbench.
 */
export function isSidebarRootConversation(
  c: Pick<DbConversationSummary, "parent_id" | "kind" | "delegation_call_id">
): boolean {
  if (c.parent_id != null) return false
  if (c.kind === "delegate") return false
  if (c.kind === "loop") return false
  if (c.delegation_call_id != null && c.delegation_call_id !== "") return false
  return true
}
