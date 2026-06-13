"use client"

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { ChevronLeft } from "lucide-react"

import { listLoopArtifacts } from "@/lib/loops-api"
import type {
  AgentType,
  LoopArtifactRow,
  LoopInboxItemRow,
  LoopSpaceSummary,
} from "@/lib/types"
import { useLoopChanged } from "@/hooks/use-loop-changed"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { IssueList } from "@/components/loops/issue-list"
import { IssueDetail } from "@/components/loops/issue-detail"
import { InboxPanel } from "@/components/loops/inbox-panel"
import { IterationDialog } from "@/components/loops/iteration-dialog"
import { IterationList } from "@/components/loops/iteration-list"
import { ArtifactList } from "@/components/loops/artifact-list"
import { MemoryPanel } from "@/components/loops/memory-panel"
import { ArtifactDrawer } from "@/components/loops/artifact-drawer"

type SpaceTab = "issues" | "iterations" | "artifacts" | "inbox" | "memory"

/** The iteration session a `question` inbox card points at, parsed from its
 *  payload (written by the engine's question router). */
interface OpenIteration {
  conversationId: number
  connectionId: string | null
  agentType: AgentType | null
}

function openIterationFromCard(item: LoopInboxItemRow): OpenIteration | null {
  const p =
    item.payload && typeof item.payload === "object"
      ? (item.payload as Record<string, unknown>)
      : {}
  const conversationId =
    typeof p.conversation_id === "number" ? p.conversation_id : 0
  if (conversationId <= 0) return null
  return {
    conversationId,
    connectionId: typeof p.connection_id === "string" ? p.connection_id : null,
    agentType:
      typeof p.agent_type === "string" ? (p.agent_type as AgentType) : null,
  }
}

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
  const [openIteration, setOpenIteration] = useState<OpenIteration | null>(null)
  const [artifacts, setArtifacts] = useState<LoopArtifactRow[]>([])
  const [selectedArtifactId, setSelectedArtifactId] = useState<number | null>(
    null
  )

  const refreshArtifacts = useCallback(() => {
    // setState lives in the promise callback, never the synchronous effect body.
    listLoopArtifacts(space.id)
      .then(setArtifacts)
      .catch(() => {
        // non-fatal; the list's empty state covers it
      })
  }, [space.id])

  useEffect(() => {
    refreshArtifacts()
  }, [refreshArtifacts])

  useLoopChanged(() => {
    refreshArtifacts()
  }, space.id)

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
          <InboxPanel
            spaceId={space.id}
            onOpenQuestion={(item) => {
              const target = openIterationFromCard(item)
              if (target) setOpenIteration(target)
            }}
          />
        </TabsContent>

        <TabsContent
          value="iterations"
          className="min-h-0 flex-1 overflow-y-auto p-4 data-[state=inactive]:hidden"
        >
          <IterationList spaceId={space.id} />
        </TabsContent>

        <TabsContent
          value="artifacts"
          className="min-h-0 flex-1 overflow-y-auto p-4 data-[state=inactive]:hidden"
        >
          <ArtifactList
            artifacts={artifacts}
            onSelect={setSelectedArtifactId}
          />
        </TabsContent>

        <TabsContent
          value="memory"
          className="min-h-0 flex-1 overflow-hidden p-4 data-[state=inactive]:hidden"
        >
          <MemoryPanel spaceId={space.id} />
        </TabsContent>
      </Tabs>

      {openIteration && (
        <IterationDialog
          open={openIteration != null}
          onOpenChange={(o) => {
            if (!o) setOpenIteration(null)
          }}
          conversationId={openIteration.conversationId}
          connectionId={openIteration.connectionId}
          agentType={openIteration.agentType}
        />
      )}

      <ArtifactDrawer
        artifactId={selectedArtifactId}
        onClose={() => setSelectedArtifactId(null)}
      />
    </div>
  )
}
