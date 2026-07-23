import type { DbConversationSummary } from "@/lib/types"

type ConversationIdentity = Pick<
  DbConversationSummary,
  "parent_id" | "kind" | "delegation_call_id"
>

/**
 * True for multi-agent delegation children (DB markers only — not UI tree
 * depth). Worktree layout also indents ordinary sessions at depth ≥ 1; those
 * must NOT count as sub-sessions.
 */
export function isDelegationSubsession(c: ConversationIdentity): boolean {
  if (c.parent_id != null) return true
  if (c.kind === "delegate") return true
  if (c.delegation_call_id != null && c.delegation_call_id !== "") return true
  return false
}

/**
 * Whether a conversation belongs in the workspace sidebar as a top-level row
 * (folder group / Chat / Pinned).
 *
 * Delegation children nest under their parent when "Show delegated sub-sessions"
 * is on — they must never appear as peer roots under a folder. Loop rows belong
 * to the loops workbench.
 */
export function isSidebarRootConversation(c: ConversationIdentity): boolean {
  if (isDelegationSubsession(c)) return false
  if (c.kind === "loop") return false
  return true
}
