"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { Check, ChevronDown, Loader2, Server } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { ScrollArea } from "@/components/ui/scroll-area"
import { useConnection } from "@/hooks/use-connection"
import { useModelProviders } from "@/hooks/use-model-providers"
import { useTabStore } from "@/stores/tab-store"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { openSettingsWindow, updateConversationModelSelection } from "@/lib/api"
import { getModelProviderApiTypes } from "@/lib/model-provider-capabilities"
import type { ModelProviderRecord } from "@/lib/model-provider-types"
import {
  getModelProviderDraftSelection,
  clearModelProviderDraftSelection,
  setModelProviderDraftSelection,
  useModelProviderDraftSelection,
  markModelProviderRestoreSettled,
  isModelProviderRestoreSettled,
} from "@/stores/model-provider-selection-store"
import {
  loadRememberedAgentModelSelection,
  isRememberedAgentModelSelectionAvailable,
  rememberAgentModelSelection,
} from "@/lib/remembered-agent-model-selection"
import type { AgentType } from "@/lib/types"
import { cn } from "@/lib/utils"

interface ModelProviderPickerProps {
  /** The conversation's agent. Compatible providers are filtered to this
   *  agent's supported API families. */
  agentType?: AgentType | null
  /** The workspace tab whose conversation this picker reads/writes. Omit (or
   *  null) for pre-bind composers with no conversation to save into. */
  tabId?: string | null
  /** Extra classes for the trigger button (compact on mobile, etc.). */
  className?: string
  side?: "top" | "bottom"
  align?: "start" | "center" | "end"
}

/**
 * The shared Model Provider (provider → model) selector used in conversation
 * composers. Renders as a single fully expanded two-level list — every provider
 * is a static group header with all of its models visible beneath it (no
 * expand/collapse). Reads the live models.json catalog through the shared
 * `useModelProviders` store (which refetches on `model-providers://updated`),
 * filters to the conversation agent's compatible API families, and persists the
 * chosen provider/model on the conversation. Only mounted when the agent's
 * model source is "provider" (see `MessageInput`).
 */
export function ModelProviderPicker({
  agentType,
  tabId,
  className,
  side = "bottom",
  align = "start",
}: ModelProviderPickerProps) {
  const t = useTranslations("ModelProviderPicker")
  const { records } = useModelProviders()
  const [open, setOpen] = useState(false)
  const connection = useConnection(tabId ?? "__provider_picker__")
  const draftSelection = useModelProviderDraftSelection(tabId)

  const conversationId = useTabStore(
    useCallback(
      (s) => s.tabs.find((tab) => tab.id === tabId)?.conversationId ?? null,
      [tabId]
    )
  )
  const summary = useAppWorkspaceStore(
    useCallback(
      (s) =>
        conversationId != null
          ? (s.conversations.find((c) => c.id === conversationId) ?? null)
          : null,
      [conversationId]
    )
  )

  // Without a known agent (pre-bind composer) show every enabled provider;
  // with one, filter to the API families that agent can actually speak.
  const supportedApis =
    agentType != null ? new Set(getModelProviderApiTypes(agentType)) : null
  const visibleRecords = (records ?? []).filter(
    (r) => r.enabled && (supportedApis == null || supportedApis.has(r.api))
  )

  const selection =
    draftSelection ??
    (summary?.model_source === "provider" &&
    summary.model_provider_id != null &&
    summary.model_provider_model_id != null
      ? {
          providerId: summary.model_provider_id,
          modelId: summary.model_provider_model_id,
        }
      : null)

  const label = selection
    ? `${selection.providerId} · ${selection.modelId}`
    : t("placeholder")

  // A per-agent remembered choice (from a previous conversation) pre-seeds a
  // NEW draft so the user doesn't re-pick it. Seeded only once per (tab,
  // agent) key. "If it still exists" is enforced against the live catalog
  // before seeding.
  const rememberedForAgent = useMemo(
    () =>
      agentType != null ? loadRememberedAgentModelSelection(agentType) : null,
    [agentType]
  )

  useEffect(() => {
    if (tabId == null || agentType == null) return
    if (conversationId != null) return // bound conversation owns its selection
    if (draftSelection != null) return // user already made an explicit choice
    if (rememberedForAgent == null) return
    if (records == null) return // catalog not settled yet
    const key = `${tabId}:${agentType}`
    if (isModelProviderRestoreSettled(key)) return
    if (
      !isRememberedAgentModelSelectionAvailable(
        agentType,
        rememberedForAgent,
        records
      )
    ) {
      return
    }
    setModelProviderDraftSelection(tabId, rememberedForAgent)
    markModelProviderRestoreSettled(key)
  }, [
    tabId,
    agentType,
    conversationId,
    draftSelection,
    rememberedForAgent,
    records,
  ])

  const persistSelection = useCallback(
    async (providerId: string, modelId: string) => {
      if (conversationId == null) return
      const hadDraftSelection =
        getModelProviderDraftSelection(tabId ?? "") != null
      // Optimistic local update so the composer + sidebar reflect the choice
      // immediately; the backend re-broadcasts the authoritative row after save.
      const current = useAppWorkspaceStore.getState().conversations
      const row = current.find((c) => c.id === conversationId)
      if (row) {
        useAppWorkspaceStore.getState().applyConversationUpsert({
          ...row,
          model_source: "provider",
          model_provider_id: providerId,
          model_provider_model_id: modelId,
          model: modelId,
        })
      }
      try {
        await updateConversationModelSelection(
          conversationId,
          providerId,
          modelId
        )
        // The choice was actually saved to a conversation — remember it for
        // the agent so the next new conversation can restore it.
        if (agentType != null) {
          rememberAgentModelSelection(agentType, { providerId, modelId })
        }
        // A provider selection changes launch env. If this surface owns a live
        // idle connection, apply immediately; a busy owner gets the existing
        // backend staleness event/banner instead of being interrupted mid-turn.
        if (!connection.isViewer && connection.status === "connected") {
          try {
            await connection.reapplyConfig(conversationId)
          } catch (reapplyError) {
            console.warn(
              "[ModelProviderPicker] reapply config failed",
              reapplyError
            )
          }
        }
      } catch (err) {
        toast.error(t("saveFailed"), { description: String(err) })
        if (hadDraftSelection) {
          setModelProviderDraftSelection(tabId ?? "", {
            providerId,
            modelId,
          })
        }
        // Re-sync the authoritative row on failure.
        void useAppWorkspaceStore.getState().refreshConversations()
      }
      clearModelProviderDraftSelection(tabId ?? "")
    },
    [agentType, connection, conversationId, t, tabId]
  )

  const handleSelect = useCallback(
    (providerId: string, modelId: string) => {
      setOpen(false)
      if (conversationId == null) {
        setModelProviderDraftSelection(tabId ?? "", { providerId, modelId })
        return
      }
      void persistSelection(providerId, modelId)
    },
    [conversationId, persistSelection, tabId]
  )

  const handleOpenProvidersSettings = useCallback(() => {
    setOpen(false)
    void openSettingsWindow("model-providers").catch((err) => {
      console.warn("[ModelProviderPicker] open settings failed", err)
    })
  }, [])

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="xs"
          aria-label={t("openPicker")}
          className={cn(
            "min-w-0 gap-1 px-1.5 text-muted-foreground",
            selection != null && "text-foreground",
            className
          )}
        >
          <span className="max-w-[10rem] truncate">{label}</span>
          <ChevronDown className="size-3 shrink-0" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        side={side}
        align={align}
        className="w-72 max-w-[calc(100vw-1rem)] overflow-hidden p-1"
      >
        {!records ? (
          <div className="flex h-28 items-center justify-center text-muted-foreground">
            <Loader2 className="h-4 w-4 animate-spin" />
          </div>
        ) : (
          <ScrollArea className="max-h-80">
            <div className="space-y-1.5">
              {visibleRecords.map((r) => (
                <ModelProviderGroup
                  key={r.providerId}
                  record={r}
                  selection={selection}
                  onSelect={handleSelect}
                  labels={{
                    vision: t("vision"),
                    reasoning: t("reasoning"),
                    noModels: t("noModels"),
                  }}
                />
              ))}
              {visibleRecords.length === 0 && (
                <div className="flex items-center gap-1.5 px-2 py-3 text-2xs text-muted-foreground">
                  <Server className="size-3" />
                  {t("noProviders")}
                </div>
              )}
              <div className="border-t pt-1">
                <button
                  type="button"
                  onClick={handleOpenProvidersSettings}
                  className="flex w-full items-center gap-1.5 rounded-md px-2 py-1.5 text-left text-2xs text-muted-foreground hover:bg-muted/50 hover:text-foreground"
                >
                  {t("configureProviders")}
                </button>
              </div>
            </div>
          </ScrollArea>
        )}
      </PopoverContent>
    </Popover>
  )
}

function ModelProviderGroup({
  record,
  selection,
  onSelect,
  labels,
}: {
  record: ModelProviderRecord
  selection: { providerId: string; modelId: string } | null
  onSelect: (providerId: string, modelId: string) => void
  labels: { vision: string; reasoning: string; noModels: string }
}) {
  return (
    <div key={record.providerId}>
      <div className="flex w-full items-center gap-1.5 px-2 py-1">
        <span className="min-w-0 flex-1">
          <span className="block truncate text-xs font-semibold">
            {record.providerId}
          </span>
          <span className="block truncate text-2xs text-muted-foreground">
            {record.models.length} · {record.api}
          </span>
        </span>
      </div>
      <div className="ml-3 space-y-0.5 border-l pl-2">
        {record.models.map((m) => {
          const active =
            selection?.providerId === record.providerId &&
            selection?.modelId === m.id
          return (
            <button
              key={m.id}
              type="button"
              onClick={() => onSelect(record.providerId, m.id)}
              className={cn(
                "flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-left hover:bg-muted/50",
                active && "text-primary"
              )}
            >
              <span className="min-w-0 flex-1">
                <span className="block truncate text-xs font-medium">
                  {m.id}
                </span>
                <span className="block truncate text-2xs text-muted-foreground">
                  {[
                    m.input === "text-image" ? labels.vision : null,
                    m.reasoning ? labels.reasoning : null,
                    m.contextWindow ? `${m.contextWindow}` : null,
                  ]
                    .filter(Boolean)
                    .join(" · ") || "\u00a0"}
                </span>
              </span>
              {active && <Check className="size-3.5 shrink-0 text-primary" />}
            </button>
          )
        })}
        {record.models.length === 0 && (
          <div className="px-2 py-1.5 text-2xs text-muted-foreground">
            {labels.noModels}
          </div>
        )}
      </div>
    </div>
  )
}
