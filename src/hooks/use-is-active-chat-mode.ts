"use client"

import { useActiveFolder } from "@/contexts/active-folder-context"
import { useTabStore } from "@/contexts/tab-context"

/**
 * True when the active conversation is folderless "chat mode" — either a bound
 * conversation whose backing folder is a hidden chat folder
 * (`kind === "chat"`), or a not-yet-sent chat draft (the in-memory `isChat`
 * tab flag). Drives hiding of folder-bound chrome — the top-bar branch
 * selector, the aux-panel toggle, the right sidebar, and the composer branch
 * picker — consistently from the moment "no-folder mode" is selected through
 * first send and beyond.
 */
export function useIsActiveChatMode(): boolean {
  const { activeFolder } = useActiveFolder()
  const tabs = useTabStore((s) => s.tabs)
  const activeTabId = useTabStore((s) => s.activeTabId)
  if (activeFolder?.kind === "chat") return true
  const activeTab = tabs.find((t) => t.id === activeTabId)
  return activeTab?.isChat === true
}
