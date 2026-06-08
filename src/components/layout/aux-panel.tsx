"use client"

import { useCallback, useState } from "react"
import { Folder, FolderPen, GitCommit } from "lucide-react"
import { useTranslations } from "next-intl"
import {
  useAuxPanelContext,
  type AuxPanelTab,
} from "@/contexts/aux-panel-context"
import { Tabs, TabsList, TabsTrigger, TabsContent } from "@/components/ui/tabs"
import { FileTreeTab } from "./aux-panel-file-tree-tab"
import { GitChangesTab } from "./aux-panel-git-changes-tab"
import { GitLogTab } from "./aux-panel-git-log-tab"

const LAZY_TABS: AuxPanelTab[] = ["file_tree", "changes", "git_log"]

export function AuxPanel() {
  const t = useTranslations("Folder.auxPanel.tabs")
  const { isOpen, activeTab, setActiveTab } = useAuxPanelContext()
  const [mountedTabs, setMountedTabs] = useState<Set<AuxPanelTab>>(
    () => new Set(LAZY_TABS.filter((tab) => tab === activeTab))
  )

  // Ensure the active tab is mounted (covers both user clicks and programmatic changes)
  if (isOpen && LAZY_TABS.includes(activeTab) && !mountedTabs.has(activeTab)) {
    setMountedTabs((prev) => new Set(prev).add(activeTab))
  }

  const handleTabValueChange = useCallback(
    (value: string) => {
      setActiveTab(value as AuxPanelTab)
    },
    [setActiveTab]
  )

  if (!isOpen) return null

  return (
    <aside className="group/aux-panel flex h-full min-h-0 flex-col overflow-hidden bg-sidebar text-sidebar-foreground select-none">
      <Tabs
        value={activeTab}
        onValueChange={handleTabValueChange}
        className="flex h-full flex-col gap-0"
      >
        <TabsList
          variant="line"
          className="h-10 w-full shrink-0 justify-start border-b border-border px-3 group-data-horizontal/tabs:h-10"
        >
          <TabsTrigger
            value="file_tree"
            title={t("files")}
            aria-label={t("files")}
          >
            <Folder className="h-3.5 w-3.5" />
          </TabsTrigger>
          <TabsTrigger
            value="changes"
            title={t("changes")}
            aria-label={t("changes")}
          >
            <FolderPen className="h-3.5 w-3.5" />
          </TabsTrigger>
          <TabsTrigger
            value="git_log"
            title={t("commits")}
            aria-label={t("commits")}
          >
            <GitCommit className="h-3.5 w-3.5" />
          </TabsTrigger>
        </TabsList>

        <TabsContent
          value="file_tree"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          {mountedTabs.has("file_tree") ? <FileTreeTab /> : null}
        </TabsContent>
        <TabsContent
          value="changes"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          {mountedTabs.has("changes") ? <GitChangesTab /> : null}
        </TabsContent>
        <TabsContent
          value="git_log"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          {mountedTabs.has("git_log") ? <GitLogTab /> : null}
        </TabsContent>
      </Tabs>
    </aside>
  )
}
