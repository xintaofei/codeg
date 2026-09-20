"use client"

import { useEffect, useMemo, useReducer, useState } from "react"
import { useTranslations } from "next-intl"
import {
  AlertCircle,
  ArrowDown,
  ArrowUp,
  Code2,
  FlaskConical,
  ListTodo,
  RotateCcw,
  ShieldCheck,
  Sparkles,
  Trash2,
  type LucideIcon,
} from "lucide-react"
import { useAcpAgents } from "@/hooks/use-acp-agents"
import { describeAgentOptions } from "@/lib/api"
import { getAgentLabel } from "@/lib/custom-agents"
import type {
  AgentType,
  LoopBack,
  PipelineRole,
  PipelineStep,
} from "@/lib/types"
import { BUILTIN_AGENT_TYPES, STEP_ID_REGEX } from "@/lib/pipeline-graph-edit"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import { Textarea } from "@/components/ui/textarea"
import { cn } from "@/lib/utils"

export interface PipelineStepInspectorProps {
  step: PipelineStep
  loop?: LoopBack | null
  availableLoopTargets?: PipelineStep[]
  open: boolean
  onClose: () => void
  onSave: (step: PipelineStep, loop?: LoopBack | null) => void | Promise<void>
  onDeleteStep?: (stepId: string) => void
  /** Move this step one place earlier / later in the chain. Steps run in
   *  order, so this is how a planner gets in front of an existing coder. */
  onMoveStep?: (stepId: string, direction: "up" | "down") => void
  className?: string
}

const ROLE_ICONS: Record<PipelineRole, LucideIcon> = {
  planner: ListTodo,
  coder: Code2,
  reviewer: ShieldCheck,
  tests: FlaskConical,
  custom: Sparkles,
}

const READ_ONLY_SUPPORTED_AGENTS = new Set<string>([
  "claude_code",
  "codex",
  "gemini",
])

export function PipelineStepInspector({
  step,
  loop,
  availableLoopTargets = [],
  open,
  onClose,
  onSave,
  onDeleteStep,
  onMoveStep,
  className,
}: PipelineStepInspectorProps) {
  const t = useTranslations("Pipeline")
  const { agents } = useAcpAgents()

  interface StepState {
    id: string
    label: string
    role: PipelineRole
    agentType: string
    model: string
    readOnly: boolean
    promptTemplate: string
    timeoutMinutes: number
    readMemory: boolean
    enableLoop: boolean
    loopTarget: string
    maxIterations: number
    validationError: string | null
  }

  const getInitialState = useMemo(
    () => (): StepState => ({
      id: step.id,
      label: step.label,
      role: step.role,
      agentType: step.agent_type,
      model: step.config_values?.model ?? "",
      readOnly: step.read_only,
      promptTemplate: step.prompt_template,
      timeoutMinutes: Math.max(1, Math.round(step.timeout_secs / 60)),
      readMemory: step.read_memory,
      enableLoop: Boolean(loop),
      loopTarget: loop?.to_step ?? availableLoopTargets[0]?.id ?? "",
      maxIterations: loop?.max_iterations ?? 3,
      validationError: null,
    }),
    [step, loop, availableLoopTargets]
  )

  const reducer = (
    state: StepState,
    action:
      | { type: "reset"; payload: StepState }
      | { type: "setId"; payload: string }
      | { type: "setLabel"; payload: string }
      | { type: "setRole"; payload: PipelineRole }
      | { type: "setAgentType"; payload: string }
      | { type: "setModel"; payload: string }
      | { type: "setReadOnly"; payload: boolean }
      | { type: "setPromptTemplate"; payload: string }
      | { type: "setTimeoutMinutes"; payload: number }
      | { type: "setReadMemory"; payload: boolean }
      | { type: "setEnableLoop"; payload: boolean }
      | { type: "setLoopTarget"; payload: string }
      | { type: "setMaxIterations"; payload: number }
      | { type: "setValidationError"; payload: string | null }
  ): StepState => {
    switch (action.type) {
      case "reset":
        return action.payload
      case "setId":
        return { ...state, id: action.payload }
      case "setLabel":
        return { ...state, label: action.payload }
      case "setRole":
        return { ...state, role: action.payload }
      case "setAgentType":
        return { ...state, agentType: action.payload }
      case "setModel":
        return { ...state, model: action.payload }
      case "setReadOnly":
        return { ...state, readOnly: action.payload }
      case "setPromptTemplate":
        return { ...state, promptTemplate: action.payload }
      case "setTimeoutMinutes":
        return { ...state, timeoutMinutes: action.payload }
      case "setReadMemory":
        return { ...state, readMemory: action.payload }
      case "setEnableLoop":
        return { ...state, enableLoop: action.payload }
      case "setLoopTarget":
        return { ...state, loopTarget: action.payload }
      case "setMaxIterations":
        return { ...state, maxIterations: action.payload }
      case "setValidationError":
        return { ...state, validationError: action.payload }
      default:
        return state
    }
  }

  const [state, dispatch] = useReducer(reducer, undefined, getInitialState)

  const setId = (value: string) => dispatch({ type: "setId", payload: value })
  const setLabel = (value: string) =>
    dispatch({ type: "setLabel", payload: value })
  const setRole = (value: PipelineRole) =>
    dispatch({ type: "setRole", payload: value })
  const setAgentType = (value: string) =>
    dispatch({ type: "setAgentType", payload: value })
  const setModel = (value: string) =>
    dispatch({ type: "setModel", payload: value })
  const setReadOnly = (value: boolean) =>
    dispatch({ type: "setReadOnly", payload: value })
  const setPromptTemplate = (value: string) =>
    dispatch({ type: "setPromptTemplate", payload: value })
  const setTimeoutMinutes = (value: number) =>
    dispatch({ type: "setTimeoutMinutes", payload: value })
  const setReadMemory = (value: boolean) =>
    dispatch({ type: "setReadMemory", payload: value })
  const setEnableLoop = (value: boolean) =>
    dispatch({ type: "setEnableLoop", payload: value })
  const setLoopTarget = (value: string) =>
    dispatch({ type: "setLoopTarget", payload: value })
  const setMaxIterations = (value: number) =>
    dispatch({ type: "setMaxIterations", payload: value })
  const setValidationError = (value: string | null) =>
    dispatch({ type: "setValidationError", payload: value })

  // Destructure for easier access
  const {
    id,
    label,
    role,
    agentType,
    model,
    readOnly,
    promptTemplate,
    timeoutMinutes,
    readMemory,
    enableLoop,
    loopTarget,
    maxIterations,
    validationError,
  } = state

  // Reset state when step or open changes
  useEffect(() => {
    dispatch({ type: "reset", payload: getInitialState() })
  }, [getInitialState, open])

  // What THIS agent says it accepts as a model. Probed live, like the
  // delegation defaults do, rather than hardcoded: the ids differ per agent
  // and change with the adapter version. The answer is stamped with the agent
  // it belongs to, so switching agents cannot show the previous one's models.
  const [probe, setProbe] = useState<{
    agent: string
    choices: { value: string; label: string }[]
  } | null>(null)
  useEffect(() => {
    if (!open || !agentType) return
    let cancelled = false
    void describeAgentOptions(agentType as AgentType)
      .then((snapshot) => {
        if (cancelled) return
        const models = snapshot.config_options.find(
          (o) => o.id === "model" && o.kind.type === "select"
        )
        if (!models || models.kind.type !== "select") {
          setProbe({ agent: agentType, choices: [] })
          return
        }
        // Grouped agents (Antigravity groups by family) list their options
        // only inside the groups, so both places have to be read.
        const flat = [
          ...models.kind.options,
          ...models.kind.groups.flatMap((g) => g.options),
        ]
        setProbe({
          agent: agentType,
          choices: flat.map((o) => ({
            value: o.value,
            label: o.name || o.value,
          })),
        })
      })
      .catch((e) => {
        // A probe that fails leaves the text box in place, which still works.
        console.error("[PipelineStepInspector] model probe failed:", e)
        if (!cancelled) setProbe({ agent: agentType, choices: [] })
      })
    return () => {
      cancelled = true
    }
  }, [open, agentType])

  const answered = probe?.agent === agentType
  const modelChoices = answered ? (probe?.choices ?? []) : []
  const probingModels = !answered

  const agentOptions = useMemo(() => {
    const list = agents.map((a) => a.agent_type)
    for (const b of BUILTIN_AGENT_TYPES) {
      if (!list.includes(b)) list.push(b)
    }
    if (step.agent_type && !list.includes(step.agent_type)) {
      list.push(step.agent_type)
    }
    return list
  }, [agents, step.agent_type])

  const showReadOnlyWarning =
    readOnly && !READ_ONLY_SUPPORTED_AGENTS.has(agentType)

  const canLoop =
    (role === "reviewer" || role === "tests") && availableLoopTargets.length > 0

  const handleSave = async () => {
    const trimmedId = id.trim()
    if (!trimmedId || !STEP_ID_REGEX.test(trimmedId)) {
      setValidationError(
        t("validation.badStepId", { id: trimmedId || "empty" })
      )
      return
    }

    if (!promptTemplate.trim()) {
      setValidationError(t("validation.emptyPrompt", { id: trimmedId }))
      return
    }

    if (
      timeoutMinutes < 1 ||
      timeoutMinutes > 1440 ||
      Number.isNaN(timeoutMinutes)
    ) {
      setValidationError(t("validation.badTimeout", { id: trimmedId }))
      return
    }

    const configValues: Record<string, string> = { ...step.config_values }
    if (model.trim()) {
      configValues.model = model.trim()
    } else {
      delete configValues.model
    }

    const updatedStep: PipelineStep = {
      ...step,
      id: trimmedId,
      label: label.trim() || trimmedId,
      role,
      agent_type: agentType,
      config_values: configValues,
      prompt_template: promptTemplate,
      timeout_secs: Math.max(1, Math.round(timeoutMinutes * 60)),
      read_memory: readMemory,
      read_only: readOnly,
    }

    let updatedLoop: LoopBack | null = null
    if (canLoop && enableLoop && loopTarget) {
      const iterations = Math.min(10, Math.max(1, Math.round(maxIterations)))
      updatedLoop = {
        from_step: trimmedId,
        to_step: loopTarget,
        max_iterations: iterations,
      }
    }

    setValidationError(null)
    const result = onSave(updatedStep, updatedLoop)
    if (result && typeof (result as Promise<void>).then === "function") {
      await result
    }
    onClose()
  }

  const RoleIcon = ROLE_ICONS[role] || Sparkles

  return (
    <Dialog open={open} onOpenChange={(isOpen) => !isOpen && onClose()}>
      <DialogContent
        className={cn("max-w-lg max-h-[90vh] overflow-y-auto", className)}
        aria-describedby="pipeline-step-inspector-description"
      >
        <DialogHeader>
          <div className="flex items-center gap-2">
            <div className="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary">
              <RoleIcon className="size-4" />
            </div>
            <div>
              <DialogTitle className="text-base font-semibold">
                {t("inspectorTitle")}
              </DialogTitle>
              <DialogDescription
                id="pipeline-step-inspector-description"
                className="text-xs text-muted-foreground"
              >
                {step.label || step.id} ({step.role})
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        {validationError && (
          <div className="flex items-center gap-2 rounded-lg border border-destructive/30 bg-destructive/10 p-2.5 text-xs text-destructive">
            <AlertCircle className="size-4 shrink-0" />
            <span>{validationError}</span>
          </div>
        )}

        <div className="space-y-4 py-2 text-sm">
          {/* Step ID & Label */}
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="step-id" className="text-xs font-medium">
                ID
              </Label>
              <Input
                id="step-id"
                value={id}
                onChange={(e) => setId(e.target.value.toLowerCase().trim())}
                placeholder="step_id"
                className="font-mono text-xs"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="step-label" className="text-xs font-medium">
                Label
              </Label>
              <Input
                id="step-label"
                value={label}
                onChange={(e) => setLabel(e.target.value)}
                placeholder="Display label"
                className="text-xs"
              />
            </div>
          </div>

          {/* Role */}
          <div className="space-y-1.5">
            <Label className="text-xs font-medium">{t("mode")}</Label>
            <div className="grid grid-cols-5 gap-1.5">
              {(
                [
                  ["planner", "rolePlanner", ListTodo],
                  ["coder", "roleCoder", Code2],
                  ["reviewer", "roleReviewer", ShieldCheck],
                  ["tests", "roleTests", FlaskConical],
                  ["custom", "roleCustom", Sparkles],
                ] as const
              ).map(([rKey, lKey, Icon]) => {
                const active = role === rKey
                return (
                  <button
                    key={rKey}
                    type="button"
                    onClick={() => {
                      setRole(rKey)
                      if (rKey === "reviewer") setReadOnly(true)
                    }}
                    className={cn(
                      "flex flex-col items-center justify-center gap-1 rounded-lg border p-2 text-center text-xs transition-colors",
                      active
                        ? "border-primary bg-primary/10 text-primary font-medium"
                        : "border-border/60 bg-background text-muted-foreground hover:bg-muted/50"
                    )}
                  >
                    <Icon className="size-4" />
                    <span className="text-[11px] truncate w-full">
                      {t(lKey)}
                    </span>
                  </button>
                )
              })}
            </div>
          </div>

          {/* Agent & Model */}
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="step-agent" className="text-xs font-medium">
                {t("agent")}
              </Label>
              <select
                id="step-agent"
                value={agentType}
                onChange={(e) => setAgentType(e.target.value)}
                className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-xs shadow-sm outline-none focus-visible:ring-1 focus-visible:ring-ring"
              >
                {agentOptions.map((opt) => (
                  <option key={opt} value={opt}>
                    {getAgentLabel(opt)}
                  </option>
                ))}
              </select>
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="step-model" className="text-xs font-medium">
                {t("model")}
              </Label>
              {/* A free text box here meant guessing the agent's own model id.
                  When the probe answers, pick from what the agent actually
                  accepts; fall back to typing for agents that advertise none
                  (or while the probe is still out). */}
              {modelChoices.length > 0 ? (
                <select
                  id="step-model"
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  className="flex h-9 w-full rounded-md border border-input bg-background px-3 py-1 text-xs shadow-sm outline-none focus-visible:ring-1 focus-visible:ring-ring"
                >
                  <option value="">{t("modelUnconfirmed")}</option>
                  {modelChoices.map((opt) => (
                    <option key={opt.value} value={opt.value}>
                      {opt.label}
                    </option>
                  ))}
                </select>
              ) : (
                <Input
                  id="step-model"
                  value={model}
                  onChange={(e) => setModel(e.target.value)}
                  placeholder={
                    probingModels ? t("modelProbing") : t("modelUnconfirmed")
                  }
                  className="text-xs font-mono"
                />
              )}
            </div>
          </div>

          {/* Prompt Template */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between">
              <Label htmlFor="step-prompt" className="text-xs font-medium">
                {t("prompt")}
              </Label>
              <span className="text-[11px] text-muted-foreground">
                {t("promptHint")}
              </span>
            </div>
            <Textarea
              id="step-prompt"
              rows={4}
              value={promptTemplate}
              onChange={(e) => setPromptTemplate(e.target.value)}
              placeholder="$task"
              className="font-mono text-xs leading-relaxed"
            />
          </div>

          {/* Timeout */}
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="step-timeout" className="text-xs font-medium">
                {t("timeout")}
              </Label>
              <Input
                id="step-timeout"
                type="number"
                min={1}
                max={1440}
                value={timeoutMinutes}
                onChange={(e) =>
                  setTimeoutMinutes(Number.parseInt(e.target.value, 10) || 1)
                }
                className="text-xs"
              />
            </div>

            {/* Read Memory Toggle */}
            <div className="flex flex-col justify-end space-y-1.5">
              <div className="flex h-9 items-center justify-between rounded-lg border border-border/60 px-3">
                <Label
                  htmlFor="step-read-memory"
                  className="cursor-pointer text-xs font-normal text-muted-foreground"
                >
                  {t("readMemory")}
                </Label>
                <Switch
                  id="step-read-memory"
                  checked={readMemory}
                  onCheckedChange={setReadMemory}
                />
              </div>
            </div>
          </div>

          {/* Read Only Toggle */}
          <div className="space-y-1.5">
            <div className="flex items-center justify-between rounded-lg border border-border/60 p-3">
              <div className="space-y-0.5">
                <Label
                  htmlFor="step-read-only"
                  className="cursor-pointer text-xs font-medium"
                >
                  {t("readOnly")}
                </Label>
                {showReadOnlyWarning && (
                  <p className="text-[11px] text-amber-600 dark:text-amber-400">
                    {t("readOnlyUnsupported")}
                  </p>
                )}
              </div>
              <Switch
                id="step-read-only"
                checked={readOnly}
                onCheckedChange={setReadOnly}
              />
            </div>
          </div>

          {/* Loopback section (for Reviewer / Tests) */}
          {canLoop && (
            <div className="rounded-lg border border-border/80 bg-muted/20 p-3 space-y-3">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-1.5 text-xs font-medium">
                  <RotateCcw className="size-3.5 text-primary" />
                  <span>{t("canvasLoopEdge")}</span>
                </div>
                <Switch
                  id="step-enable-loop"
                  checked={enableLoop}
                  onCheckedChange={setEnableLoop}
                />
              </div>

              {enableLoop && (
                <div className="grid grid-cols-2 gap-3 pt-1">
                  <div className="space-y-1.5">
                    <Label
                      htmlFor="loop-target"
                      className="text-xs text-muted-foreground"
                    >
                      Target step
                    </Label>
                    <select
                      id="loop-target"
                      value={loopTarget}
                      onChange={(e) => setLoopTarget(e.target.value)}
                      className="flex h-8 w-full rounded-md border border-input bg-background px-2.5 py-1 text-xs shadow-sm outline-none focus-visible:ring-1 focus-visible:ring-ring"
                    >
                      {availableLoopTargets.map((tgt) => (
                        <option key={tgt.id} value={tgt.id}>
                          {tgt.label || tgt.id} ({tgt.role})
                        </option>
                      ))}
                    </select>
                  </div>

                  <div className="space-y-1.5">
                    <Label
                      htmlFor="loop-max-iter"
                      className="text-xs text-muted-foreground"
                    >
                      {t("maxIterations")}
                    </Label>
                    <Input
                      id="loop-max-iter"
                      type="number"
                      min={1}
                      max={10}
                      value={maxIterations}
                      onChange={(e) =>
                        setMaxIterations(
                          Number.parseInt(e.target.value, 10) || 1
                        )
                      }
                      className="h-8 text-xs"
                    />
                  </div>
                </div>
              )}
            </div>
          )}
        </div>

        <DialogFooter className="flex items-center justify-between sm:justify-between gap-2 pt-2 border-t border-border/50">
          <div className="flex items-center gap-1">
            {onDeleteStep && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                onClick={() => {
                  onDeleteStep(step.id)
                  onClose()
                }}
                className="text-destructive hover:bg-destructive/10 hover:text-destructive h-8 px-2 text-xs"
              >
                <Trash2 className="size-3.5 mr-1" />
                Delete
              </Button>
            )}
            {onMoveStep && (
              <>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  aria-label={t("moveEarlier")}
                  title={t("moveEarlier")}
                  onClick={() => onMoveStep(step.id, "up")}
                  className="h-8 w-8 p-0"
                >
                  <ArrowUp className="size-3.5" />
                </Button>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  aria-label={t("moveLater")}
                  title={t("moveLater")}
                  onClick={() => onMoveStep(step.id, "down")}
                  className="h-8 w-8 p-0"
                >
                  <ArrowDown className="size-3.5" />
                </Button>
              </>
            )}
          </div>

          <div className="flex items-center gap-2">
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={onClose}
              className="h-8 text-xs"
            >
              Cancel
            </Button>
            <Button
              type="button"
              size="sm"
              onClick={handleSave}
              className="h-8 text-xs"
            >
              Save
            </Button>
          </div>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
