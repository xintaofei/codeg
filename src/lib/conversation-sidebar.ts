import type { DbConversationSummary } from "@/lib/types"

/**
 * The DB markers that identify a conversation row's place in the sidebar tree.
 * Deliberately excludes UI-side data (tree depth, expansion state): the
 * predicates below must stay decidable from the row alone.
 */
type ConversationIdentity = Pick<
  DbConversationSummary,
  "parent_id" | "kind" | "delegation_call_id"
>

/**
 * True for a multi-agent delegation child — a row spawned by a parent
 * conversation's `delegate_to_agent`. Matches any of the three DB markers the
 * backend stamps together (`create_with_delegation` keeps the invariant
 * `parent_id set ⟺ kind=delegate ⟺ delegation_call_id set`; each is checked
 * independently so a partially-written row still reads as a child).
 *
 * Reads DB markers ONLY — never a UI tree depth. Worktree layout indents
 * ORDINARY root conversations at depth ≥ 1 (a repo's root sub-group and its
 * worktree buckets under "Show worktree folders"), so a `depth > 0` term would
 * mislabel plain sessions as delegated children (the regression upstream
 * PR #375 shipped and then fixed in `1ad6f8f1`).
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
 * Delegation children nest under their parent instead, so they must never
 * appear as peer roots; `loop` rows belong to the loops workbench (mirroring
 * the backend `list_all` filter).
 */
export function isSidebarRootConversation(c: ConversationIdentity): boolean {
  if (isDelegationSubsession(c)) return false
  if (c.kind === "loop") return false
  return true
}
