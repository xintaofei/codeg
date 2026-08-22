import type { AgentType } from "@/lib/types"

export const CLOSED_TAB_STACK_LIMIT = 20

export type ClosedConversationTab = {
  kind: "conversation"
  folderId: number
  conversationId: number | null
  agentType: AgentType
  title: string
  workingDir?: string
  isPinned: boolean
}

export type ClosedFileTab = {
  kind: "file"
  path: string
  folderId: number | null
}

export type ClosedWorkspaceTab = ClosedConversationTab | ClosedFileTab

let stack: ClosedWorkspaceTab[] = []

export function pushClosedTab(tab: ClosedWorkspaceTab): void {
  stack.push(tab)
  if (stack.length > CLOSED_TAB_STACK_LIMIT) {
    stack = stack.slice(-CLOSED_TAB_STACK_LIMIT)
  }
}

export function popClosedTab(): ClosedWorkspaceTab | null {
  return stack.pop() ?? null
}

export function peekClosedTab(): ClosedWorkspaceTab | null {
  return stack[stack.length - 1] ?? null
}

export function resetClosedTabStackForTests(): void {
  stack = []
}

export function snapshotConversationTab(tab: {
  folderId: number
  conversationId: number | null
  agentType: AgentType
  title: string
  workingDir?: string
  isPinned: boolean
}): ClosedConversationTab {
  return {
    kind: "conversation",
    folderId: tab.folderId,
    conversationId: tab.conversationId,
    agentType: tab.agentType,
    title: tab.title,
    workingDir: tab.workingDir,
    isPinned: tab.isPinned,
  }
}

export function snapshotFileTab(tab: {
  path: string | null
  folderId: number | null
}): ClosedFileTab | null {
  if (!tab.path) return null
  return { kind: "file", path: tab.path, folderId: tab.folderId }
}
