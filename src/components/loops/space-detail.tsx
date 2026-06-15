"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { ChevronLeft, Settings2 } from "lucide-react"

import { listLoopArtifacts } from "@/lib/loops-api"
import type {
  AgentType,
  LoopArtifactRow,
  LoopInboxItemRow,
  LoopSpaceSummary,
} from "@/lib/types"
import { useLoopResource } from "@/hooks/use-loop-resource"
import { useLoopNav } from "@/hooks/use-loop-nav"
import type { LoopTab } from "@/lib/loop-nav"
import { Button } from "@/components/ui/button"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { useLoopOverlays } from "@/components/loops/loop-overlays-context"
import { IssueList } from "@/components/loops/issue-list"
import { IssueDetail } from "@/components/loops/issue-detail"
import { InboxPanel } from "@/components/loops/inbox-panel"
import { IterationList } from "@/components/loops/iteration-list"
import { ArtifactList } from "@/components/loops/artifact-list"
import { MemoryPanel } from "@/components/loops/memory-panel"
import { SpaceDefaultsDialog } from "@/components/loops/space-defaults-dialog"

/** The iteration session a `question` inbox card points at, parsed from its
 *  payload (written by the engine's question router). The dialog self-discovers
 *  the live connection by `conversationId`; the card's `agent_type` is kept only
 *  as a fast-path hint shown while the conversation summary loads. */
interface OpenIteration {
  conversationId: number
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
  const tDefaults = useTranslations("Loops.spaceDefaults")
  const { nav, setTab, selectIssue, openArtifact } = useLoopNav()
  const { openIteration } = useLoopOverlays()
  const tab: LoopTab = nav.tab
  const selectedIssueId = nav.issue
  const [defaultsOpen, setDefaultsOpen] = useState(false)

  // The space-wide artifact list, kept live by the realtime provider.
  const { data: artifacts } = useLoopResource<LoopArtifactRow[]>(
    () => listLoopArtifacts(space.id),
    { match: (e) => e.space_id === space.id, initial: [], deps: [space.id] }
  )

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
        onValueChange={(v) => setTab(v as LoopTab)}
        className="flex min-h-0 flex-1 flex-col"
      >
        <div className="mx-4 mt-2 flex items-center gap-2 self-start">
          <TabsList>
            <TabsTrigger value="issues">{t("tabIssues")}</TabsTrigger>
            <TabsTrigger value="iterations">{t("tabIterations")}</TabsTrigger>
            <TabsTrigger value="artifacts">{t("tabArtifacts")}</TabsTrigger>
            <TabsTrigger value="inbox">{t("tabInbox")}</TabsTrigger>
            <TabsTrigger value="memory">{t("tabMemory")}</TabsTrigger>
          </TabsList>
          <Button
            size="icon"
            variant="ghost"
            className="h-8 w-8"
            onClick={() => setDefaultsOpen(true)}
          >
            <Settings2 className="h-4 w-4" />
            <span className="sr-only">{tDefaults("button")}</span>
          </Button>
        </div>

        <TabsContent
          value="issues"
          className="min-h-0 flex-1 overflow-hidden data-[state=inactive]:hidden"
        >
          <div className="flex h-full min-h-0">
            <div className="flex w-80 shrink-0 flex-col border-r">
              <IssueList
                spaceId={space.id}
                selectedIssueId={selectedIssueId}
                onSelectIssue={selectIssue}
              />
            </div>
            <div className="min-w-0 flex-1">
              <IssueDetail
                issueId={selectedIssueId}
                spaceDefaultConfig={space.default_config}
              />
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
              if (target)
                openIteration({
                  conversationId: target.conversationId,
                  agentType: target.agentType,
                })
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
          <ArtifactList artifacts={artifacts} onSelect={openArtifact} />
        </TabsContent>

        <TabsContent
          value="memory"
          className="min-h-0 flex-1 overflow-hidden p-4 data-[state=inactive]:hidden"
        >
          <MemoryPanel spaceId={space.id} />
        </TabsContent>
      </Tabs>

      <SpaceDefaultsDialog
        spaceId={space.id}
        current={space.default_config}
        open={defaultsOpen}
        onOpenChange={setDefaultsOpen}
      />
    </div>
  )
}
