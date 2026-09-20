"use client"

import {
  useCallback,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react"
import { useTranslations } from "next-intl"
import {
  Bot,
  ChevronRight,
  Code2,
  FlaskConical,
  ListTodo,
  RotateCcw,
  ShieldCheck,
  SlidersHorizontal,
  Sparkles,
  Users,
  Workflow,
  type LucideIcon,
} from "lucide-react"

import { cn } from "@/lib/utils"
import type {
  LoopBack,
  PipelineGraph,
  PipelineModeKey,
  PipelineRole,
  PipelineStep,
} from "@/lib/types"
import {
  DEFAULT_PIPELINE_MODE,
  loadPipelineMode,
  savePipelineMode,
} from "@/lib/pipeline-mode-storage"
import { pipelinePresets } from "@/lib/api"

export interface PipelineModeSwitchProps {
  /** The bound folder id. Used for localStorage key scoping. */
  folderId?: number | null
  /** Selected pipeline mode (controlled). */
  mode?: PipelineModeKey
  /** Default mode if uncontrolled (defaults to "single"). */
  defaultMode?: PipelineModeKey
  /** Callback fired when the mode selection changes. */
  onModeChange?: (mode: PipelineModeKey) => void
  /**
   * Optional graph definition. When supplied, its steps and loops are
   * displayed as role chips and loop limit indicator. If omitted, standard
   * defaults for duet / team are synthesized.
   */
  graph?: PipelineGraph | null
  /**
   * Per-role overrides for agent type and model (e.g. from user delegation config).
   */
  agentDefaults?: Record<
    string,
    { agentType?: string; model?: string | null }
  > | null
  /** Whether the switcher controls are disabled. */
  disabled?: boolean
  /** Optional callback to open custom pipeline configuration / builder. */
  onConfigureCustom?: () => void
  /** Optional class name for the root wrapper element. */
  className?: string
  /** Whether to render role chips and loop limit indicator. Defaults to true. */
  showChips?: boolean
}

interface ModeOption {
  key: PipelineModeKey
  labelKey: "modeSingle" | "modeDuet" | "modeTeam" | "modeCustom"
  hintKey?: "modeHintDuet" | "modeHintTeam"
  Icon: LucideIcon
}

const MODE_OPTIONS: readonly ModeOption[] = [
  { key: "single", labelKey: "modeSingle", Icon: Bot },
  { key: "duet", labelKey: "modeDuet", hintKey: "modeHintDuet", Icon: Users },
  {
    key: "team",
    labelKey: "modeTeam",
    hintKey: "modeHintTeam",
    Icon: Workflow,
  },
  { key: "custom", labelKey: "modeCustom", Icon: SlidersHorizontal },
] as const

const ROLE_ICONS: Record<PipelineRole, LucideIcon> = {
  planner: ListTodo,
  coder: Code2,
  reviewer: ShieldCheck,
  tests: FlaskConical,
  custom: Sparkles,
}

function getRoleLabelKey(
  role: PipelineRole
): "rolePlanner" | "roleCoder" | "roleReviewer" | "roleTests" | "roleCustom" {
  switch (role) {
    case "planner":
      return "rolePlanner"
    case "coder":
      return "roleCoder"
    case "reviewer":
      return "roleReviewer"
    case "tests":
      return "roleTests"
    case "custom":
    default:
      return "roleCustom"
  }
}

function getStepModel(
  step: PipelineStep,
  agentDefaults?: Record<
    string,
    { agentType?: string; model?: string | null }
  > | null
): string | null {
  if (step.config_values?.model && step.config_values.model.trim().length > 0) {
    return step.config_values.model.trim()
  }
  const defaultEntry = agentDefaults?.[step.role] ?? agentDefaults?.[step.id]
  if (defaultEntry?.model && defaultEntry.model.trim().length > 0) {
    return defaultEntry.model.trim()
  }
  return null
}

function getStepAgent(
  step: PipelineStep,
  agentDefaults?: Record<
    string,
    { agentType?: string; model?: string | null }
  > | null
): string | null {
  if (
    step.agent_type &&
    step.agent_type !== "default" &&
    step.agent_type !== "coder" &&
    step.agent_type !== "planner" &&
    step.agent_type !== "reviewer" &&
    step.agent_type !== "tests"
  ) {
    return step.agent_type
  }
  const defaultEntry = agentDefaults?.[step.role] ?? agentDefaults?.[step.id]
  if (defaultEntry?.agentType && defaultEntry.agentType !== "default") {
    return defaultEntry.agentType
  }
  return null
}

/**
 * PipelineModeSwitch renders the composer's multi-agent pipeline mode selector
 * ("Single agent" | "Duet" | "Team" | "Custom").
 *
 * For pipeline modes ("duet", "team", "custom"), it also renders role chips
 * indicating the execution sequence (agent + model) and the loop round limit.
 * If a role's model is unconfigured, it displays `Pipeline.modelUnconfirmed`.
 */
export function PipelineModeSwitch({
  folderId,
  mode: controlledMode,
  defaultMode = DEFAULT_PIPELINE_MODE,
  onModeChange,
  graph,
  agentDefaults,
  disabled = false,
  onConfigureCustom,
  className,
  showChips = true,
}: PipelineModeSwitchProps): ReactNode {
  const t = useTranslations("Pipeline")

  const [internalMode, setInternalMode] = useState<PipelineModeKey>(() => {
    if (controlledMode !== undefined) {
      return controlledMode
    }
    return loadPipelineMode(folderId) ?? defaultMode
  })

  // Synchronize internal state during render when folderId changes (uncontrolled mode)
  const [prevFolderId, setPrevFolderId] = useState(folderId)
  if (prevFolderId !== folderId) {
    setPrevFolderId(folderId)
    if (controlledMode === undefined) {
      setInternalMode(loadPipelineMode(folderId) ?? defaultMode)
    }
  }

  const [presets, setPresets] = useState<Record<string, PipelineGraph> | null>(
    null
  )
  useEffect(() => {
    let cancelled = false
    void (async () => {
      try {
        const list = await pipelinePresets()
        if (cancelled) return
        const map: Record<string, PipelineGraph> = {}
        for (const preset of list) {
          // Keyed by preset_key ("duet", "team") — `name` is the display
          // label ("Duet"), which never matches the lookups below.
          map[preset.preset_key ?? preset.name] = preset.graph
        }
        setPresets(map)
      } catch (e) {
        console.error("[PipelineModeSwitch] failed to load presets:", e)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])

  const effectiveMode = controlledMode ?? internalMode

  const handleSelectMode = useCallback(
    (newMode: PipelineModeKey) => {
      if (disabled) return
      if (controlledMode === undefined) {
        setInternalMode(newMode)
      }
      savePipelineMode(newMode, folderId)
      onModeChange?.(newMode)
    },
    [controlledMode, disabled, folderId, onModeChange]
  )

  const activeSteps = useMemo<PipelineStep[]>(() => {
    if (effectiveMode === "single") return []
    if (effectiveMode === "custom" && graph?.steps && graph.steps.length > 0) {
      return graph.steps
    }
    if (presets && effectiveMode === "duet") {
      return presets.duet?.steps ?? []
    }
    if (presets && effectiveMode === "team") {
      return presets.team?.steps ?? []
    }
    return []
  }, [effectiveMode, graph, presets])

  const activeLoops = useMemo<LoopBack[]>(() => {
    if (effectiveMode === "single") return []
    if (effectiveMode === "custom" && graph?.loops && graph.loops.length > 0) {
      return graph.loops
    }
    if (presets && effectiveMode === "duet") {
      return presets.duet?.loops ?? []
    }
    if (presets && effectiveMode === "team") {
      return presets.team?.loops ?? []
    }
    return []
  }, [effectiveMode, graph, presets])

  const maxLoopIterations = useMemo<number>(() => {
    if (activeLoops.length === 0) return 0
    return Math.max(...activeLoops.map((l) => l.max_iterations))
  }, [activeLoops])

  return (
    <div
      className={cn("flex flex-col gap-1.5", className)}
      data-testid="pipeline-mode-switch"
    >
      {/* Segmented Mode Switcher */}
      <div className="flex items-center gap-1.5">
        <div
          role="radiogroup"
          aria-label={t("title")}
          className="inline-flex items-center gap-0.5 rounded-lg border border-border/40 bg-muted/40 p-0.5 text-xs"
        >
          {MODE_OPTIONS.map(({ key, labelKey, Icon }) => {
            const isSelected = effectiveMode === key
            const labelText = t(labelKey)

            return (
              <button
                key={key}
                type="button"
                role="radio"
                aria-checked={isSelected}
                aria-label={labelText}
                disabled={disabled}
                data-mode={key}
                data-state={isSelected ? "active" : "inactive"}
                onClick={() => handleSelectMode(key)}
                className={cn(
                  "inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-xs font-medium transition-colors cursor-pointer select-none",
                  isSelected
                    ? "bg-background text-foreground shadow-xs"
                    : "text-muted-foreground hover:bg-muted/70 hover:text-foreground",
                  disabled && "pointer-events-none opacity-50"
                )}
              >
                <Icon className="size-3.5 shrink-0" />
                <span>{labelText}</span>
              </button>
            )
          })}
        </div>

        {/* Custom configuration action button */}
        {effectiveMode === "custom" && onConfigureCustom ? (
          <button
            type="button"
            aria-label={t("inspectorTitle")}
            onClick={onConfigureCustom}
            disabled={disabled}
            className="inline-flex size-6 items-center justify-center rounded-md border border-border/40 bg-muted/40 text-muted-foreground transition-colors hover:bg-muted hover:text-foreground cursor-pointer focus-visible:ring-1 focus-visible:ring-ring"
          >
            <SlidersHorizontal className="size-3" />
          </button>
        ) : null}
      </div>

      {/* Role Chips & Loop Limit (shown in multi-agent pipeline modes) */}
      {showChips &&
      effectiveMode !== "single" &&
      activeSteps.length > 0 &&
      (effectiveMode === "custom" || presets) ? (
        <div
          className="flex flex-wrap items-center gap-1.5 text-xs text-muted-foreground"
          data-testid="pipeline-mode-chips"
        >
          {activeSteps.map((step, index) => {
            const RoleIcon = ROLE_ICONS[step.role] ?? Sparkles
            const roleLabel = t(getRoleLabelKey(step.role))
            const modelName = getStepModel(step, agentDefaults)
            const agentName = getStepAgent(step, agentDefaults)

            return (
              <div key={step.id} className="inline-flex items-center gap-1.5">
                {index > 0 ? (
                  <ChevronRight className="size-3 shrink-0 text-muted-foreground/40" />
                ) : null}

                <div
                  className="inline-flex items-center gap-1.5 rounded-md border border-border/50 bg-background/80 px-2 py-0.5 text-xs font-medium shadow-2xs"
                  data-testid={`pipeline-step-chip-${step.role}`}
                >
                  <RoleIcon className="size-3 text-muted-foreground shrink-0" />
                  <span>{roleLabel}</span>
                  {agentName ? (
                    <>
                      <span className="text-muted-foreground/40">·</span>
                      <span className="text-2xs text-muted-foreground">
                        {agentName}
                      </span>
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
                </div>
              </div>
            )
          })}

          {/* Loop limit badge */}
          {maxLoopIterations > 0 ? (
            <div
              className="inline-flex items-center gap-1 rounded-md border border-border/40 bg-muted/40 px-2 py-0.5 text-2xs font-normal text-muted-foreground"
              data-testid="pipeline-loop-limit-chip"
            >
              <RotateCcw className="size-2.5 shrink-0 text-muted-foreground/70" />
              <span>{t("loopLimit", { n: maxLoopIterations })}</span>
            </div>
          ) : null}
        </div>
      ) : null}
    </div>
  )
}
