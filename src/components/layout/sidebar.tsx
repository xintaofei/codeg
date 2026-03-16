"use client"

import { useCallback, useRef } from "react"
import {
  ChevronsDownUp,
  ChevronsUpDown,
  Crosshair,
  ListTree,
  MessageSquareText,
  Plus,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { useFolderContext } from "@/contexts/folder-context"
import { useTabContext } from "@/contexts/tab-context"
import { useSidebarContext } from "@/contexts/sidebar-context"
import {
  SidebarConversationList,
  type SidebarConversationListHandle,
} from "@/components/conversations/sidebar-conversation-list"
import { SidebarDirectoryTab } from "@/components/layout/sidebar-directory-tab"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"

export function Sidebar() {
  const t = useTranslations("Folder.sidebar")
  const { folder } = useFolderContext()
  const { openNewConversationTab } = useTabContext()
  const { isOpen, activeTab, setActiveTab } = useSidebarContext()
  const listRef = useRef<SidebarConversationListHandle>(null)

  const handleNewConversation = useCallback(() => {
    if (!folder) return
    openNewConversationTab("codex", folder.path)
  }, [folder, openNewConversationTab])

  if (!isOpen) return null

  return (
    <aside className="group/sidebar flex h-full min-h-0 flex-col overflow-hidden bg-sidebar text-sidebar-foreground select-none">
      <Tabs
        value={activeTab}
        onValueChange={(value) =>
          setActiveTab(value as "conversations" | "directory")
        }
        className="flex h-full flex-col gap-0"
      >
        <div className="flex h-10 items-center justify-between border-b border-border px-2.5">
          <TabsList
            variant="line"
            className="h-full justify-start gap-1 border-0 px-0 group-data-horizontal/tabs:h-full"
          >
            <TabsTrigger
              value="conversations"
              title={t("title")}
              aria-label={t("title")}
              className="h-full flex-none gap-1.5 rounded-none border-b-2 border-transparent px-2.5 text-[12px] font-medium text-muted-foreground data-active:border-foreground/60 data-active:bg-transparent data-active:text-foreground after:hidden"
            >
              <MessageSquareText className="h-3.5 w-3.5" />
              {t("title")}
            </TabsTrigger>
            <TabsTrigger
              value="directory"
              title={t("tabs.directory")}
              aria-label={t("tabs.directory")}
              className="h-full flex-none gap-1.5 rounded-none border-b-2 border-transparent px-2.5 text-[12px] font-medium text-muted-foreground data-active:border-foreground/60 data-active:bg-transparent data-active:text-foreground after:hidden"
            >
              <ListTree className="h-3.5 w-3.5" />
              {t("tabs.directory")}
            </TabsTrigger>
          </TabsList>

          {activeTab === "conversations" ? (
            <div className="flex items-center gap-0.5 opacity-60 transition-opacity group-hover/sidebar:opacity-100">
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6 shrink-0 rounded-md text-muted-foreground"
                onClick={() => listRef.current?.scrollToActive()}
                title={t("locateActiveConversation")}
              >
                <Crosshair className="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6 shrink-0 rounded-md text-muted-foreground"
                onClick={() => listRef.current?.expandAll()}
                title={t("expandAllGroups")}
              >
                <ChevronsUpDown className="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6 shrink-0 rounded-md text-muted-foreground"
                onClick={() => listRef.current?.collapseAll()}
                title={t("collapseAllGroups")}
              >
                <ChevronsDownUp className="h-3.5 w-3.5" />
              </Button>
              <Button
                variant="ghost"
                size="icon"
                className="h-6 w-6 shrink-0 rounded-md text-muted-foreground"
                onClick={handleNewConversation}
                title={t("newConversation")}
              >
                <Plus className="h-3.5 w-3.5" />
              </Button>
            </div>
          ) : (
            <div className="w-0" />
          )}
        </div>

        <TabsContent
          value="conversations"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          <SidebarConversationList ref={listRef} />
        </TabsContent>

        <TabsContent
          value="directory"
          forceMount
          className="mt-0 flex-1 min-h-0 overflow-hidden"
        >
          <SidebarDirectoryTab />
        </TabsContent>
      </Tabs>
    </aside>
  )
}
