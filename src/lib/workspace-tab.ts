import type { AgentType, ConversationStatus } from "@/lib/types"

interface WorkspaceTabBase {
  id: string
  folderId: number
  title: string
  isPinned: boolean
}

export interface ConversationWorkspaceTab extends WorkspaceTabBase {
  kind: "conversation"
  conversationId: number | null
  /** Runtime key used before a draft binds to its persisted conversation. */
  runtimeConversationId?: number
  agentType: AgentType
  workingDir?: string
  status?: ConversationStatus
  agentTypeProvisional?: boolean
  isChat?: boolean
}

/** Device-local view placement for a persisted PK round. Round execution and
 * data remain owned by the PK store/database. */
export interface PkWorkspaceTab extends WorkspaceTabBase {
  kind: "pk"
  roundId: string
}

export type WorkspaceTab = ConversationWorkspaceTab | PkWorkspaceTab

export function isConversationWorkspaceTab(
  tab: WorkspaceTab
): tab is ConversationWorkspaceTab {
  return tab.kind === "conversation"
}

export function isConversationDraft(
  tab: WorkspaceTab
): tab is ConversationWorkspaceTab & { conversationId: null } {
  return tab.kind === "conversation" && tab.conversationId == null
}
