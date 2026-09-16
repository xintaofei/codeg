"use client"

/**
 * Hand a conversation to another agent in place.
 *
 * One dialog, one action. The user picks the agent, optionally says what it
 * should focus on, reads which path the backend will take, and confirms. The
 * backend (`acp::handoff`) stops the current agent, moves the transcript or
 * seeds a briefing, spawns the target and moves the conversation row; this
 * dialog then swaps the open tab onto the new agent so the same conversation
 * reopens under it. A failure leaves the row and the tab exactly where they
 * were, which is why the tab swap only runs after the backend returned.
 *
 * The path statement is fetched from the backend for the chosen target rather
 * than guessed here: only the backend knows which agent homes exist, whether
 * the source transcript is still on disk, and how long the briefing would be.
 */

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { AgentSelector } from "@/components/chat/agent-selector"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Textarea } from "@/components/ui/textarea"
import { useTabActions } from "@/contexts/tab-context"
import { acpHandoff, acpHandoffPlan, type HandoffPlan } from "@/lib/api"
import { getAgentLabel } from "@/lib/custom-agents"
import { getSavedPrefsForConnect } from "@/lib/selector-prefs-storage"
import { TurnBusyError } from "@/lib/turn-busy"
import type { AgentType } from "@/lib/types"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { useConversationRuntimeActions } from "@/stores/conversation-runtime-store"

interface AgentHandoffDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  conversationId: number
  folderId: number
  sourceAgentType: AgentType
  title?: string | null
}

function errorMessage(err: unknown): string {
  if (err instanceof Error) return err.message
  if (typeof err === "string") return err
  if (err && typeof err === "object") {
    const message = (err as { message?: unknown }).message
    if (typeof message === "string") return message
  }
  return String(err)
}

export function AgentHandoffDialog({
  open,
  onOpenChange,
  conversationId,
  folderId,
  sourceAgentType,
  title,
}: AgentHandoffDialogProps) {
  const t = useTranslations("Folder.chat.agentHandoff")
  const { openTab, closeConversationTab } = useTabActions()
  const refreshConversations = useAppWorkspaceStore(
    (s) => s.refreshConversations
  )
  const { setExternalId, refetchDetail } = useConversationRuntimeActions()

  const [target, setTarget] = useState<AgentType | null>(null)
  const [note, setNote] = useState("")
  const [plan, setPlan] = useState<HandoffPlan | null>(null)
  const [planLoading, setPlanLoading] = useState(false)
  const [planError, setPlanError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  // A plan request that lands after the target moved on must not describe
  // the wrong agent.
  const planRequestRef = useRef(0)

  useEffect(() => {
    if (!open) {
      setTarget(null)
      setNote("")
      setPlan(null)
      setPlanError(null)
      setError(null)
      setBusy(false)
    }
  }, [open])

  useEffect(() => {
    // No plan for the agent the conversation already runs on: the answer is
    // known here, and the backend would only say the same.
    if (!open || !target || target === sourceAgentType) {
      setPlan(null)
      return
    }
    const requestId = ++planRequestRef.current
    setPlanLoading(true)
    setPlanError(null)
    setPlan(null)
    acpHandoffPlan(conversationId, target)
      .then((next) => {
        if (planRequestRef.current !== requestId) return
        setPlan(next)
      })
      .catch((err: unknown) => {
        if (planRequestRef.current !== requestId) return
        setPlanError(errorMessage(err))
      })
      .finally(() => {
        if (planRequestRef.current === requestId) setPlanLoading(false)
      })
  }, [open, target, conversationId, sourceAgentType])

  const targetLabel = target ? getAgentLabel(target) : ""
  const sourceLabel = getAgentLabel(sourceAgentType)

  const blockedText = (() => {
    if (!target) return null
    if (target === sourceAgentType)
      return t("blockedSameAgent", { target: targetLabel })
    if (!plan?.blocked) return null
    switch (plan.blocked) {
      case "same_agent":
        return t("blockedSameAgent", { target: targetLabel })
      case "no_session":
        return t("blockedNoSession")
      case "not_installed":
        return t("blockedNotInstalled", { target: targetLabel })
      case "disabled":
        return t("blockedDisabled", { target: targetLabel })
      default:
        return t("blockedGeneric", {
          target: targetLabel,
          message: plan.blockedMessage ?? plan.blocked,
        })
    }
  })()

  const planText = (() => {
    if (!target || target === sourceAgentType) return null
    if (planLoading) return t("planLoading")
    if (planError)
      return t("blockedGeneric", { target: targetLabel, message: planError })
    if (!plan || plan.blocked) return null
    if (plan.path === "native") return t("planNative", { target: targetLabel })
    if (plan.briefingTruncated) {
      return t("planSummaryTruncated", { target: targetLabel })
    }
    return t("planSummary", {
      target: targetLabel,
      turns: plan.turnCount,
      verbatim: plan.verbatimTurns,
    })
  })()

  const canConfirm =
    !!target &&
    target !== sourceAgentType &&
    !!plan &&
    !plan.blocked &&
    !planLoading &&
    !planError &&
    !busy

  const handleConfirm = useCallback(async () => {
    if (!target || !canConfirm) return
    setBusy(true)
    setError(null)
    try {
      // The target agent's own saved selector preferences, read from the
      // same store a normal connect uses: the source agent's model never
      // travels with the conversation.
      const prefs = getSavedPrefsForConnect(target)
      const trimmed = note.trim()
      const result = await acpHandoff(
        conversationId,
        target,
        trimmed.length > 0 ? trimmed : null,
        prefs.modeId,
        prefs.configValues
      )
      // The row now names the new agent + session. Point the runtime at the
      // new session before any tab reconnects, then swap the tab: open the
      // new one first so closing the old cannot spawn a replacement draft.
      setExternalId(conversationId, result.externalId)
      await refreshConversations()
      openTab(
        result.folderId,
        conversationId,
        result.toAgentType,
        false,
        title ?? undefined
      )
      closeConversationTab(folderId, conversationId, sourceAgentType)
      refetchDetail(conversationId)
      toast.success(t("success", { target: getAgentLabel(result.toAgentType) }))
      onOpenChange(false)
    } catch (err: unknown) {
      const message =
        err instanceof TurnBusyError
          ? t("busy")
          : t("failed", { message: errorMessage(err) })
      setError(message)
      toast.error(message)
    } finally {
      setBusy(false)
    }
  }, [
    target,
    canConfirm,
    note,
    conversationId,
    folderId,
    sourceAgentType,
    title,
    setExternalId,
    refreshConversations,
    openTab,
    closeConversationTab,
    refetchDetail,
    t,
    onOpenChange,
  ])

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        if (busy) return
        onOpenChange(next)
      }}
    >
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("dialogTitle")}</DialogTitle>
          <DialogDescription>{t("dialogDescription")}</DialogDescription>
        </DialogHeader>
        <div className="flex flex-col gap-4">
          <div className="flex flex-col gap-1.5">
            <span className="text-sm font-medium">{t("targetLabel")}</span>
            <AgentSelector
              defaultAgentType={target ?? undefined}
              onSelect={setTarget}
              onFallback={setTarget}
              disabled={busy}
            />
          </div>
          <div className="flex flex-col gap-1.5">
            <label className="text-sm font-medium" htmlFor="agent-handoff-note">
              {t("noteLabel")}
            </label>
            <Textarea
              id="agent-handoff-note"
              value={note}
              onChange={(e) => setNote(e.target.value)}
              placeholder={t("notePlaceholder")}
              disabled={busy}
              rows={3}
            />
          </div>
          {blockedText ? (
            <p role="alert" className="text-sm text-destructive">
              {blockedText}
            </p>
          ) : planText ? (
            <p
              data-slot="agent-handoff-plan"
              className="rounded-md border border-border/60 bg-muted/30 px-3 py-2 text-sm text-muted-foreground"
            >
              {planText}
              {plan?.nativeReason === "transcript_missing" ? (
                <> {t("planNativeUnavailable", { source: sourceLabel })}</>
              ) : null}
            </p>
          ) : null}
          {error ? (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          ) : null}
        </div>
        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={busy}
          >
            {t("cancel")}
          </Button>
          <Button onClick={() => void handleConfirm()} disabled={!canConfirm}>
            {busy ? t("working") : t("confirm")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
