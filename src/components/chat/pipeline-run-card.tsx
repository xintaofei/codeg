"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import {
  AlertCircle,
  AlertTriangle,
  Ban,
  Check,
  Code,
  HelpCircle,
  Loader2,
  MessageSquare,
  Square,
  Workflow,
  XCircle,
} from "lucide-react"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { getAgentLabel } from "@/lib/custom-agents"
import { cn } from "@/lib/utils"
import type {
  AgentType,
  PipelineAttempt,
  PipelineRole,
  PipelineRun,
  PipelineRunStatus,
  PipelineStep,
  PipelineVerdict,
} from "@/lib/types"

export interface PipelineRunCardProps {
  run: PipelineRun
  onStop?: (runId: number) => void | Promise<void>
  onOpenCode?: (runId: number) => void
  onOpenConversation?: (conversationId: number) => void
  onApply?: (
    runId: number,
    strategy: "squash" | "no_ff"
  ) => void | Promise<void>
  isStopping?: boolean
  className?: string
}

function getStatusConfig(
  status: PipelineRunStatus,
  t: ReturnType<typeof useTranslations<"Pipeline">>
) {
  switch (status) {
    case "running":
      return {
        label: t("running"),
        variant: "outline" as const,
        className: "border-primary/40 bg-primary/10 text-primary",
        icon: Loader2,
        animate: true,
      }
    case "succeeded":
      return {
        label: t("statusSucceeded"),
        variant: "outline" as const,
        className:
          "border-emerald-500/40 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
        icon: Check,
        animate: false,
      }
    case "failed":
      return {
        label: t("statusFailed"),
        variant: "destructive" as const,
        className: "",
        icon: XCircle,
        animate: false,
      }
    case "cancelled":
      return {
        label: t("statusCancelled"),
        variant: "secondary" as const,
        className: "text-muted-foreground",
        icon: Ban,
        animate: false,
      }
    case "interrupted":
      return {
        label: t("statusInterrupted"),
        variant: "outline" as const,
        className:
          "border-amber-500/40 bg-amber-500/10 text-amber-600 dark:text-amber-400",
        icon: AlertTriangle,
        animate: false,
      }
    case "stopped_max_iterations":
      return {
        label: t("statusStoppedMaxIterations"),
        variant: "outline" as const,
        className:
          "border-orange-500/40 bg-orange-500/10 text-orange-600 dark:text-orange-400",
        icon: AlertCircle,
        animate: false,
      }
    case "inconclusive":
      return {
        label: t("statusInconclusive"),
        variant: "outline" as const,
        className:
          "border-muted-foreground/40 bg-muted/40 text-muted-foreground",
        icon: HelpCircle,
        animate: false,
      }
  }
}

function getRoleLabel(
  role: PipelineRole,
  t: ReturnType<typeof useTranslations<"Pipeline">>
) {
  switch (role) {
    case "planner":
      return t("rolePlanner")
    case "coder":
      return t("roleCoder")
    case "reviewer":
      return t("roleReviewer")
    case "tests":
      return t("roleTests")
    case "custom":
      return t("roleCustom")
  }
}

function getVerdictLabel(
  verdict: PipelineVerdict,
  t: ReturnType<typeof useTranslations<"Pipeline">>
) {
  switch (verdict) {
    case "pass":
      return t("verdictPass")
    case "changes_requested":
      return t("verdictChangesRequested")
    case "inconclusive":
      return t("verdictInconclusive")
  }
}

export function PipelineRunCard({
  run,
  onStop,
  onOpenCode,
  onOpenConversation,
  onApply,
  isStopping = false,
  className,
}: PipelineRunCardProps) {
  const t = useTranslations("Pipeline")
  const [localStopping, setLocalStopping] = useState(false)
  const [applying, setApplying] = useState(false)

  const effectiveStopping = isStopping || localStopping
  const steps = run.graph?.steps ?? []
  const totalSteps = Math.max(steps.length, 1)

  let currentStepIndex = 0
  if (run.current_step_id) {
    const foundIdx = steps.findIndex((s) => s.id === run.current_step_id)
    if (foundIdx >= 0) {
      currentStepIndex = foundIdx
    }
  } else if (run.attempts.length > 0) {
    const lastAttempt = run.attempts[run.attempts.length - 1]
    const foundIdx = steps.findIndex((s) => s.id === lastAttempt.step_id)
    if (foundIdx >= 0) {
      currentStepIndex = foundIdx
    }
  }

  const currentStep: PipelineStep | undefined = steps[currentStepIndex]

  const currentIteration = Math.max(
    run.current_iteration,
    run.attempts.length > 0
      ? run.attempts[run.attempts.length - 1].iteration
      : 1,
    1
  )

  const relevantLoop =
    (currentStep &&
      run.graph?.loops?.find((l) => l.from_step === currentStep.id)) ||
    run.graph?.loops?.[0]
  const maxIterations = relevantLoop?.max_iterations ?? 3

  const latestAttempt: PipelineAttempt | undefined =
    run.attempts.length > 0 ? run.attempts[run.attempts.length - 1] : undefined

  const agentType = currentStep?.agent_type || ""
  const agentLabel = agentType ? getAgentLabel(agentType as AgentType) : ""

  const actualModel = latestAttempt?.model_actual || null
  const modelDisplay = actualModel || t("modelUnconfirmed")

  const attemptWithVerdict = [...run.attempts]
    .reverse()
    .find((a) => a.verdict !== null)
  const verdict = attemptWithVerdict?.verdict ?? null
  const verdictSource = attemptWithVerdict?.verdict_source ?? null
  const reviewerNotes = attemptWithVerdict?.notes?.trim() ?? null

  const activeConversationId =
    latestAttempt?.conversation_id ?? run.parent_conversation_id ?? null

  const statusCfg = getStatusConfig(run.status, t)
  const StatusIcon = statusCfg.icon

  const handleStop = async () => {
    if (!onStop || effectiveStopping) return
    setLocalStopping(true)
    try {
      await onStop(run.id)
    } finally {
      setLocalStopping(false)
    }
  }

  const handleApply = async (strategy: "squash" | "no_ff" = "squash") => {
    if (!onApply || applying) return
    setApplying(true)
    try {
      await onApply(run.id, strategy)
    } finally {
      setApplying(false)
    }
  }

  const showActions = Boolean(
    (run.status === "running" && onStop) ||
    (onOpenConversation && activeConversationId !== null) ||
    onOpenCode ||
    (onApply && run.status === "succeeded")
  )

  return (
    <div
      className={cn(
        "w-full space-y-3 rounded-lg border border-border/60 bg-card/60 p-4 text-sm",
        className
      )}
    >
      <div className="flex items-center justify-between gap-2">
        <div className="flex items-center gap-2 font-medium text-foreground">
          <Workflow className="size-4 shrink-0 text-muted-foreground" />
          <span>{t("title")}</span>
        </div>
        <Badge variant={statusCfg.variant} className={statusCfg.className}>
          <StatusIcon
            className={cn(
              "size-3 shrink-0",
              statusCfg.animate && "animate-spin"
            )}
          />
          <span>{statusCfg.label}</span>
        </Badge>
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
        <span className="font-medium text-foreground">
          {t("step", { index: currentStepIndex + 1, total: totalSteps })}
        </span>
        <span>•</span>
        <span>
          {t("iteration", { n: currentIteration, max: maxIterations })}
        </span>
        {agentLabel && (
          <>
            <span>•</span>
            <span>
              {t("agent")}:{" "}
              <span className="text-foreground">{agentLabel}</span>
            </span>
          </>
        )}
        <span>•</span>
        <span>
          {t("model")}: <span className="text-foreground">{modelDisplay}</span>
        </span>
      </div>

      {steps.length > 0 && (
        <div className="flex flex-wrap items-center gap-1.5 pt-0.5">
          {steps.map((step, idx) => {
            const isCurrent = idx === currentStepIndex
            const isPast = idx < currentStepIndex
            return (
              <div
                key={step.id || idx}
                className={cn(
                  "flex items-center gap-1 rounded-md px-2 py-0.5 text-xs transition-colors",
                  isCurrent
                    ? "border border-primary/30 bg-primary/15 font-medium text-primary"
                    : isPast
                      ? "bg-muted text-muted-foreground"
                      : "bg-muted/40 text-muted-foreground/60"
                )}
              >
                <span>{step.label || getRoleLabel(step.role, t)}</span>
              </div>
            )
          })}
        </div>
      )}

      {verdict && (
        <div className="flex items-center gap-2 rounded-md border border-border/40 bg-muted/20 p-2.5 text-xs">
          <span className="font-medium text-muted-foreground">
            {t("canvasNodeVerdict")}:
          </span>
          <Badge
            variant="outline"
            className={cn(
              verdict === "pass" &&
                "border-emerald-500/40 bg-emerald-500/10 text-emerald-600 dark:text-emerald-400",
              verdict === "changes_requested" &&
                "border-amber-500/40 bg-amber-500/10 text-amber-600 dark:text-amber-400",
              verdict === "inconclusive" &&
                "border-muted-foreground/40 bg-muted/40 text-muted-foreground"
            )}
          >
            {verdict === "pass" && <Check className="mr-1 size-3 shrink-0" />}
            {verdict === "changes_requested" && (
              <AlertTriangle className="mr-1 size-3 shrink-0" />
            )}
            {verdict === "inconclusive" && (
              <HelpCircle className="mr-1 size-3 shrink-0" />
            )}
            {getVerdictLabel(verdict, t)}
          </Badge>
          {verdictSource === "marker" && (
            <span className="italic text-muted-foreground/80">
              ({t("verdictSourceMarker")})
            </span>
          )}
          {verdictSource === "guard" && (
            <span className="italic text-muted-foreground/80">
              ({t("verdictSourceGuard")})
            </span>
          )}
        </div>
      )}

      {reviewerNotes && (
        <div className="space-y-1 rounded-md border border-border/60 bg-muted/30 p-3 text-xs">
          <div className="font-medium text-foreground">
            {t("notesForCoder")}
          </div>
          <div className="leading-relaxed whitespace-pre-wrap text-muted-foreground">
            {reviewerNotes}
          </div>
        </div>
      )}

      {run.error && (
        <div className="flex items-start gap-2 rounded-md border border-destructive/40 bg-destructive/10 p-3 text-xs text-destructive">
          <AlertCircle className="mt-0.5 size-4 shrink-0" />
          <div className="leading-relaxed">{run.error}</div>
        </div>
      )}

      {showActions && (
        <div className="flex flex-wrap items-center justify-end gap-2 border-t border-border/40 pt-2">
          {run.status === "running" && onStop && (
            <Button
              variant="destructive"
              size="sm"
              disabled={effectiveStopping}
              onClick={handleStop}
              className="h-7 px-2.5 text-xs"
            >
              {effectiveStopping ? (
                <Loader2 className="mr-1 size-3.5 shrink-0 animate-spin" />
              ) : (
                <Square className="mr-1 size-3.5 shrink-0" />
              )}
              {effectiveStopping ? t("stopping") : t("stop")}
            </Button>
          )}

          {onOpenConversation && activeConversationId !== null && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => onOpenConversation(activeConversationId)}
              className="h-7 px-2.5 text-xs"
            >
              <MessageSquare className="mr-1 size-3.5 shrink-0" />
              {t("openConversation")}
            </Button>
          )}

          {onOpenCode && (
            <Button
              variant="outline"
              size="sm"
              onClick={() => onOpenCode(run.id)}
              className="h-7 px-2.5 text-xs"
            >
              <Code className="mr-1 size-3.5 shrink-0" />
              {t("openCode")}
            </Button>
          )}

          {onApply && run.status === "succeeded" && (
            <Button
              variant="default"
              size="sm"
              disabled={applying}
              onClick={() => void handleApply("squash")}
              className="h-7 px-2.5 text-xs"
            >
              {applying ? (
                <Loader2 className="mr-1 size-3.5 shrink-0 animate-spin" />
              ) : (
                <Check className="mr-1 size-3.5 shrink-0" />
              )}
              {t("apply")}
            </Button>
          )}
        </div>
      )}
    </div>
  )
}
