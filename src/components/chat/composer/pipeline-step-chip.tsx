"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"
import { Trash2, type LucideIcon } from "lucide-react"

import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { Label } from "@/components/ui/label"
import { useAcpAgents } from "@/hooks/use-acp-agents"
import { useAgentModels } from "@/hooks/use-agent-models"
import { getAgentLabel } from "@/lib/custom-agents"
import { BUILTIN_AGENT_TYPES } from "@/lib/pipeline-graph-edit"
import type { PipelineStep } from "@/lib/types"
import { cn } from "@/lib/utils"

export interface PipelineStepChipProps {
  step: PipelineStep
  RoleIcon: LucideIcon
  roleLabel: string
  /** Agent slug shown when the step names one. */
  agentName: string | null
  /** Model id shown when the step names one. */
  modelName: string | null
  /** Absent makes the chip a plain label — the built-in chains render that way
   *  until the composer is wired to persist an edit. */
  onChange?: (patch: { agentType?: string; model?: string }) => void
  /** Absent hides the delete action (a preset's shape is fixed, and the last
   *  step of a custom chain cannot go). */
  onDelete?: () => void
  disabled?: boolean
}

const SELECT_CLASS =
  "flex h-8 w-full rounded-md border border-input bg-background px-2 text-xs shadow-2xs outline-none focus-visible:ring-1 focus-visible:ring-ring"

/**
 * One role in the composer's chain, editable in place.
 *
 * Only the two things a chain is actually re-pointed by live here — which
 * agent runs the step and on which model. Prompt, timeout and the read-only
 * flag stay in the canvas inspector: putting them in the composer would turn a
 * two-click change into a form.
 */
export function PipelineStepChip({
  step,
  RoleIcon,
  roleLabel,
  agentName,
  modelName,
  onChange,
  onDelete,
  disabled = false,
}: PipelineStepChipProps) {
  const t = useTranslations("Pipeline")
  const { agents } = useAcpAgents()
  const editable = !!onChange && !disabled
  const { choices, probing } = useAgentModels(step.agent_type, editable)

  const agentOptions = useMemo(() => {
    const list = agents.map((a) => a.agent_type)
    for (const b of BUILTIN_AGENT_TYPES) if (!list.includes(b)) list.push(b)
    if (step.agent_type && !list.includes(step.agent_type)) {
      list.push(step.agent_type)
    }
    return list
  }, [agents, step.agent_type])

  const current = step.config_values?.model ?? ""
  // A model the agent no longer lists (renamed, or the adapter changed) must
  // stay visible and selected rather than silently becoming the first option.
  const missing = current && !choices.some((c) => c.value === current)

  const body = (
    <span
      className={cn(
        "inline-flex items-center gap-1.5 rounded-md border border-border/50 bg-background/80 px-2 py-0.5 text-xs font-medium shadow-2xs",
        editable && "cursor-pointer hover:border-border hover:bg-muted/50"
      )}
      data-testid={`pipeline-step-chip-${step.role}`}
    >
      <RoleIcon className="size-3 shrink-0 text-muted-foreground" />
      <span>{roleLabel}</span>
      {agentName ? (
        <>
          <span className="text-muted-foreground/40">·</span>
          <span className="text-2xs text-muted-foreground">{agentName}</span>
        </>
      ) : null}
      <span className="text-muted-foreground/40">·</span>
      {modelName ? (
        <span className="font-mono text-2xs text-foreground/80">
          {modelName}
        </span>
      ) : (
        <span className="text-2xs font-normal text-amber-600 dark:text-amber-400">
          {t("modelUnconfirmed")}
        </span>
      )}
    </span>
  )

  if (!editable) return body

  return (
    <Popover>
      <PopoverTrigger asChild>
        <button
          type="button"
          aria-label={`${roleLabel} — ${t("stepSettings")}`}
        >
          {body}
        </button>
      </PopoverTrigger>
      <PopoverContent align="start" side="top" className="w-60 space-y-2 p-2">
        <div className="space-y-1">
          <Label htmlFor={`chip-agent-${step.id}`} className="text-2xs">
            {t("agent")}
          </Label>
          <select
            id={`chip-agent-${step.id}`}
            className={SELECT_CLASS}
            value={step.agent_type}
            onChange={(e) =>
              // Agent and model move together: the model id of the old agent
              // almost never exists on the new one, and saving them in two
              // steps would persist that invalid pair in between.
              onChange?.({ agentType: e.target.value, model: "" })
            }
          >
            {agentOptions.map((a) => (
              <option key={a} value={a}>
                {getAgentLabel(a)}
              </option>
            ))}
          </select>
        </div>

        <div className="space-y-1">
          <Label htmlFor={`chip-model-${step.id}`} className="text-2xs">
            {t("model")}
          </Label>
          <select
            id={`chip-model-${step.id}`}
            className={SELECT_CLASS}
            value={current}
            disabled={probing && choices.length === 0}
            onChange={(e) => onChange?.({ model: e.target.value })}
          >
            <option value="">
              {probing && choices.length === 0
                ? t("modelProbing")
                : t("modelAgentDefault")}
            </option>
            {missing ? (
              <option value={current}>
                {current} — {t("modelUnavailable")}
              </option>
            ) : null}
            {choices.map((c) => (
              <option key={c.value} value={c.value}>
                {c.label}
              </option>
            ))}
          </select>
        </div>

        {onDelete ? (
          <button
            type="button"
            onClick={onDelete}
            className="inline-flex w-full items-center gap-1.5 rounded-md px-2 py-1 text-2xs text-destructive hover:bg-destructive/10"
          >
            <Trash2 className="size-3" />
            {t("deleteStep")}
          </button>
        ) : null}
      </PopoverContent>
    </Popover>
  )
}
