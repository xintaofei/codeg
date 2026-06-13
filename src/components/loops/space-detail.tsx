"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { ChevronLeft } from "lucide-react"

import type { LoopSpaceSummary } from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { IssueList } from "@/components/loops/issue-list"
import { IssueDetail } from "@/components/loops/issue-detail"
import { InboxPanel } from "@/components/loops/inbox-panel"

type SpaceTab = "issues" | "iterations" | "artifacts" | "inbox" | "memory"

export function SpaceDetail({
  space,
  onBack,
}: {
  space: LoopSpaceSummary
  onBack: () => void
}) {
  const t = useTranslations("Loops.spaceDetail")
  const [tab, setTab] = useState<SpaceTab>("issues")
  const [selectedIssueId, setSelectedIssueId] = useState<number | null>(null)

  return (
    <div className="flex h-full min-h-0 flex-col overflow-hidden">
      <div className="flex shrink-0 items-center gap-2 border-b px-4 py-2.5">
        <Button
          variant="ghost"
          size="sm"
          onClick={onBack}
          className="h-7 gap-1 px-2 text-muted-foreground"
        >
          <ChevronLeft className="h-4 w-4" />
          {t("back")}
        </Button>
        <span className="text-muted-foreground/50">/</span>
        <span className="truncate font-medium">{space.name}</span>
      </div>

      <Tabs
        value={tab}
        onValueChange={(v) => setTab(v as SpaceTab)}
        className="flex min-h-0 flex-1 flex-col"
      >
        <TabsList className="mx-4 mt-2 self-start">
          <TabsTrigger value="issues">{t("tabIssues")}</TabsTrigger>
          <TabsTrigger value="iterations">{t("tabIterations")}</TabsTrigger>
          <TabsTrigger value="artifacts">{t("tabArtifacts")}</TabsTrigger>
          <TabsTrigger value="inbox">{t("tabInbox")}</TabsTrigger>
          <TabsTrigger value="memory">{t("tabMemory")}</TabsTrigger>
        </TabsList>

        <TabsContent
          value="issues"
          className="min-h-0 flex-1 overflow-hidden data-[state=inactive]:hidden"
        >
          <div className="flex h-full min-h-0">
            <div className="flex w-80 shrink-0 flex-col border-r">
              <IssueList
                spaceId={space.id}
                selectedIssueId={selectedIssueId}
                onSelectIssue={setSelectedIssueId}
              />
            </div>
            <div className="min-w-0 flex-1">
              <IssueDetail issueId={selectedIssueId} />
            </div>
          </div>
        </TabsContent>

        <TabsContent
          value="inbox"
          className="min-h-0 flex-1 overflow-hidden data-[state=inactive]:hidden"
        >
          <InboxPanel spaceId={space.id} />
        </TabsContent>

        {(["iterations", "artifacts", "memory"] as const).map((key) => (
          <TabsContent
            key={key}
            value={key}
            className="min-h-0 flex-1 overflow-hidden data-[state=inactive]:hidden"
          >
            <div className="flex h-full items-center justify-center text-sm text-muted-foreground">
              {t("comingSoon")}
            </div>
          </TabsContent>
        ))}
      </Tabs>
    </div>
  )
}
