"use client"

import { type ReactNode, useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2, Plus, X } from "lucide-react"

import { describeAgentOptions } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import {
  AGENT_LABELS,
  type AgentOptionsSnapshot,
  type AgentSpec,
  type AgentType,
  type IssueConfig,
  type LoopIssueRoute,
  type LoopStage,
  type ReviewerEntry,
  type SessionConfigOptionInfo,
} from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"

export const AGENT_TYPES: AgentType[] = [
  "claude_code",
  "codex",
  "open_code",
  "gemini",
  "open_claw",
  "cline",
  "hermes",
]

// Stages that run a single agent (one sub-tab each). `review` is special — it
// runs the configured reviewers list (its own sub-tab) instead of one agent.
const SINGLE_STAGES: LoopStage[] = [
  "triage",
  "refine",
  "design",
  "plan",
  "implement",
  "finalize",
]

// Sub-tab trigger order inside the Agents tab (after "default"): the pipeline in
// execution order, with `review` sitting right after `implement` since review
// acts on implementation output. Drives only the tab order; the review sub-tab's
// body is the reviewers list, the rest are single-agent stages.
const STAGE_SUBTABS: LoopStage[] = [
  "triage",
  "refine",
  "design",
  "plan",
  "implement",
  "review",
  "finalize",
]

// Select can't carry an empty value, so these sentinels stand in for "no value".
const INHERIT = "__inherit__" // a stage with no per-stage agent override
const ROUTE_AUTO = "__auto__" // force_route = null (triage decides)
// Mode/config "use the agent's own default" (clears the override).
const DEFAULT_SENTINEL = "__codeg_default__"

/** Form-state shape of an {@link AgentSpec}: agent + optional startup mode/config.
 *  `mode_id` is normalized to `null` (never undefined) for controlled selects. */
export interface AgentSpecForm {
  agent: AgentType
  mode_id: string | null
  config_values: Record<string, string>
}

/** A reviewer in form state: a concrete agent spec, or "use default agent"
 *  (`{ inherit: true }`), which resolves at dispatch to the issue's default
 *  review agent — mirroring how a single stage can defer to the default. */
export type ReviewerForm = AgentSpecForm | { inherit: true }

const isInheritReviewer = (r: ReviewerForm): r is { inherit: true } =>
  "inherit" in r

/**
 * Form-state mirror of `IssueConfig`. Numeric fields are kept as strings so a
 * field can be cleared / typed through intermediate values without snapping to
 * a parsed number; `formStateToConfig` serializes on save. The per-stage agents
 * (default + each single stage) carry full mode/config; the review stage uses
 * the structured `reviewers` list. The per-issue total token budget is NOT here
 * — it lives outside `IssueConfig`, owned by the issue-settings host (rendered
 * into the Limits tab via `limitsExtra`).
 */
export interface LoopConfigFormState {
  configVersion: number
  defaultSpec: AgentSpecForm
  /** Per single-stage override, or `INHERIT` to follow the default. Keyed by the
   *  stages in {@link SINGLE_STAGES} (the review stage is not here). */
  stageSpecs: Record<string, AgentSpecForm | typeof INHERIT>
  validationCommands: string[]
  reviewers: ReviewerForm[]
  /** Preserved pass-through legacy fallback; used only when `reviewers` is empty. */
  reviewerCount: number
  reviewPassRule: string
  maxAttempts: string
  autoMerge: boolean
  forceRoute: string
  iterationTimeoutSecs: string
  tokenBudgetPerTurn: string
  stallAlertSecs: string
}

function intField(n: number | null | undefined): string {
  return n == null ? "" : String(n)
}

/** Empty / non-positive → null (unlimited); otherwise the floored integer. */
function parsePositiveOrNull(s: string): number | null {
  const n = Number(s.trim())
  return Number.isFinite(n) && n > 0 ? Math.floor(n) : null
}

/** A bounded integer field with a fallback when blank or unparseable. */
function parseCount(s: string, min: number, fallback: number): number {
  const n = Number(s.trim())
  return Number.isFinite(n) ? Math.max(min, Math.floor(n)) : fallback
}

/** Wire `AgentSpec` → controlled form shape (mode_id normalized to null). */
function toSpecForm(s: AgentSpec): AgentSpecForm {
  return {
    agent: s.agent,
    mode_id: s.mode_id ?? null,
    config_values: { ...s.config_values },
  }
}

/** Form shape → wire `AgentSpec` (omit mode_id when unset, like the backend). */
function specFromForm(s: AgentSpecForm): AgentSpec {
  return {
    agent: s.agent,
    ...(s.mode_id ? { mode_id: s.mode_id } : {}),
    config_values: s.config_values,
  }
}

export function configToFormState(c: IssueConfig): LoopConfigFormState {
  const stageSpecs: Record<string, AgentSpecForm | typeof INHERIT> = {}
  for (const s of SINGLE_STAGES) {
    const spec = c.agents[s]
    stageSpecs[s] = spec ? toSpecForm(spec) : INHERIT
  }
  const route = c.force_route
  return {
    configVersion: c.v ?? 1,
    defaultSpec: toSpecForm(
      c.agents.default ?? { agent: "claude_code", config_values: {} }
    ),
    stageSpecs,
    validationCommands: [...c.validation_commands],
    reviewers: (c.reviewers ?? []).map<ReviewerForm>((r) =>
      "inherit" in r
        ? { inherit: true }
        : {
            agent: r.agent,
            mode_id: r.mode_id ?? null,
            config_values: { ...r.config_values },
          }
    ),
    reviewerCount: c.reviewer_count ?? 1,
    reviewPassRule: c.review_pass_rule || "unanimous",
    maxAttempts: String(c.max_attempts ?? 0),
    autoMerge: !!c.auto_merge,
    forceRoute: route && route !== "undecided" ? route : ROUTE_AUTO,
    iterationTimeoutSecs: intField(c.iteration_timeout_secs),
    tokenBudgetPerTurn: intField(c.token_budget_per_turn),
    stallAlertSecs: intField(c.stall_alert_secs),
  }
}

export function formStateToConfig(form: LoopConfigFormState): IssueConfig {
  const agents: Record<string, AgentSpec> = {
    default: specFromForm(form.defaultSpec),
  }
  for (const s of SINGLE_STAGES) {
    const v = form.stageSpecs[s]
    if (v !== INHERIT) agents[s] = specFromForm(v)
  }
  const reviewers: ReviewerEntry[] = form.reviewers.map((r) =>
    isInheritReviewer(r)
      ? { inherit: true }
      : {
          agent: r.agent,
          ...(r.mode_id ? { mode_id: r.mode_id } : {}),
          config_values: r.config_values,
        }
  )
  return {
    v: form.configVersion,
    agents,
    validation_commands: form.validationCommands
      .map((s) => s.trim())
      .filter(Boolean),
    // The explicit reviewer list is authoritative when present; otherwise keep
    // the legacy count so pre-`reviewers` issues round-trip unchanged.
    reviewer_count:
      reviewers.length > 0 ? reviewers.length : form.reviewerCount,
    review_pass_rule:
      form.reviewPassRule === "majority" ? "majority" : "unanimous",
    max_attempts: parseCount(form.maxAttempts, 0, 0),
    auto_merge: form.autoMerge,
    force_route:
      form.forceRoute === ROUTE_AUTO
        ? null
        : (form.forceRoute as LoopIssueRoute),
    iteration_timeout_secs: parsePositiveOrNull(form.iterationTimeoutSecs),
    token_budget_per_turn: parsePositiveOrNull(form.tokenBudgetPerTurn),
    reviewers,
    stall_alert_secs: parsePositiveOrNull(form.stallAlertSecs),
  }
}

/**
 * Tabbed editor for an `IssueConfig`. The "Agents" tab nests sub-tabs for the
 * default agent, each single-agent stage (with full mode/config, or "use
 * default"), and the review stage (reviewers list + pass rule). Validation and
 * Limits follow. Controlled — the host owns the `LoopConfigFormState` and
 * re-seeds it (e.g. on dialog open). `limitsExtra` lets a host append a field to
 * the Limits tab (the issue dialog uses it for the per-issue total budget, which
 * the space-defaults dialog has no concept of). Shared by both dialogs.
 */
export function LoopConfigForm({
  value,
  onChange,
  disabled,
  limitsExtra,
}: {
  value: LoopConfigFormState
  onChange: (next: LoopConfigFormState) => void
  disabled?: boolean
  limitsExtra?: ReactNode
}) {
  const t = useTranslations("Loops.issueSettings")
  const tStage = useTranslations("Loops.stage")
  const tRoute = useTranslations("Loops.route")
  const tCfg = useTranslations("Loops.config")

  const patch = (p: Partial<LoopConfigFormState>) =>
    onChange({ ...value, ...p })

  const setCommand = (i: number, next: string) => {
    const commands = [...value.validationCommands]
    commands[i] = next
    patch({ validationCommands: commands })
  }
  const addCommand = () =>
    patch({ validationCommands: [...value.validationCommands, ""] })
  const removeCommand = (i: number) =>
    patch({
      validationCommands: value.validationCommands.filter((_, j) => j !== i),
    })

  return (
    <Tabs defaultValue="agents" className="flex flex-col">
      <TabsList className="self-start">
        <TabsTrigger value="agents">{tCfg("tabAgents")}</TabsTrigger>
        <TabsTrigger value="validation">{tCfg("tabValidation")}</TabsTrigger>
        <TabsTrigger value="limits">{tCfg("tabLimits")}</TabsTrigger>
      </TabsList>

      <div className="mt-3 max-h-[52vh] overflow-y-auto pr-1">
        {/* Agents — nested sub-tabs: default + single stages + review */}
        <TabsContent value="agents" className="data-[state=inactive]:hidden">
          <Tabs defaultValue="default" className="flex flex-col">
            <TabsList className="h-auto flex-wrap self-start">
              <TabsTrigger value="default">{tCfg("subtabDefault")}</TabsTrigger>
              {STAGE_SUBTABS.map((s) => (
                <TabsTrigger key={s} value={s}>
                  {tStage(s)}
                </TabsTrigger>
              ))}
            </TabsList>

            <div className="mt-3">
              <TabsContent
                value="default"
                className="space-y-2 data-[state=inactive]:hidden"
              >
                <Label>{t("defaultAgent")}</Label>
                <StageAgentEditor
                  spec={value.defaultSpec}
                  allowInherit={false}
                  onChange={(next) => {
                    if (next !== INHERIT) patch({ defaultSpec: next })
                  }}
                  disabled={disabled}
                />
              </TabsContent>

              {SINGLE_STAGES.map((s) => (
                <TabsContent
                  key={s}
                  value={s}
                  className="space-y-2 data-[state=inactive]:hidden"
                >
                  <StageAgentEditor
                    spec={value.stageSpecs[s]}
                    allowInherit
                    onChange={(next) =>
                      patch({
                        stageSpecs: { ...value.stageSpecs, [s]: next },
                      })
                    }
                    disabled={disabled}
                  />
                </TabsContent>
              ))}

              <TabsContent
                value="review"
                className="space-y-4 data-[state=inactive]:hidden"
              >
                <ReviewersEditor
                  value={value.reviewers}
                  onChange={(reviewers) => patch({ reviewers })}
                  disabled={disabled}
                />
                <div className="space-y-1.5">
                  <Label htmlFor="pass-rule">{t("reviewPassRule")}</Label>
                  <div id="pass-rule">
                    <Select
                      value={value.reviewPassRule}
                      onValueChange={(v) => patch({ reviewPassRule: v })}
                      disabled={disabled}
                    >
                      <SelectTrigger className="h-8">
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="unanimous">
                          {t("ruleUnanimous")}
                        </SelectItem>
                        <SelectItem value="majority">
                          {t("ruleMajority")}
                        </SelectItem>
                      </SelectContent>
                    </Select>
                  </div>
                </div>
              </TabsContent>
            </div>
          </Tabs>
        </TabsContent>

        {/* Validation */}
        <TabsContent
          value="validation"
          className="space-y-2 data-[state=inactive]:hidden"
        >
          <Label>{t("validationCommands")}</Label>
          <p className="text-xs text-muted-foreground">{t("validationHint")}</p>
          <div className="space-y-2">
            {value.validationCommands.map((cmd, i) => (
              <div key={i} className="flex gap-2">
                <Input
                  value={cmd}
                  onChange={(e) => setCommand(i, e.target.value)}
                  placeholder={t("commandPlaceholder")}
                  className="h-8 font-mono text-xs"
                  disabled={disabled}
                />
                <Button
                  type="button"
                  variant="ghost"
                  size="icon"
                  className="h-8 w-8 shrink-0"
                  onClick={() => removeCommand(i)}
                  disabled={disabled}
                >
                  <X className="h-4 w-4" />
                </Button>
              </div>
            ))}
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-8"
              onClick={addCommand}
              disabled={disabled}
            >
              <Plus className="mr-1 h-3.5 w-3.5" />
              {t("addCommand")}
            </Button>
          </div>
        </TabsContent>

        {/* Limits */}
        <TabsContent
          value="limits"
          className="space-y-4 data-[state=inactive]:hidden"
        >
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="max-attempts">{t("maxAttempts")}</Label>
              <Input
                id="max-attempts"
                type="number"
                min={0}
                value={value.maxAttempts}
                onChange={(e) => patch({ maxAttempts: e.target.value })}
                className="h-8"
                disabled={disabled}
              />
              <p className="text-xs text-muted-foreground">
                {t("maxAttemptsHint")}
              </p>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="force-route">{t("forceRoute")}</Label>
              <div id="force-route">
                <Select
                  value={value.forceRoute}
                  onValueChange={(v) => patch({ forceRoute: v })}
                  disabled={disabled}
                >
                  <SelectTrigger className="h-8">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value={ROUTE_AUTO}>{t("routeAuto")}</SelectItem>
                    <SelectItem value="full">{tRoute("full")}</SelectItem>
                    <SelectItem value="skip_design">
                      {tRoute("skip_design")}
                    </SelectItem>
                    <SelectItem value="direct">{tRoute("direct")}</SelectItem>
                  </SelectContent>
                </Select>
              </div>
            </div>
          </div>

          <div className="flex items-center justify-between gap-3">
            <div>
              <Label htmlFor="auto-merge">{t("autoMerge")}</Label>
              <p className="text-xs text-muted-foreground">
                {t("autoMergeHint")}
              </p>
            </div>
            <Switch
              id="auto-merge"
              checked={value.autoMerge}
              onCheckedChange={(v) => patch({ autoMerge: v })}
              disabled={disabled}
            />
          </div>

          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="iter-timeout">{t("iterationTimeout")}</Label>
              <Input
                id="iter-timeout"
                type="number"
                min={1}
                value={value.iterationTimeoutSecs}
                onChange={(e) =>
                  patch({ iterationTimeoutSecs: e.target.value })
                }
                placeholder={t("unlimitedPlaceholder")}
                className="h-8"
                disabled={disabled}
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="per-turn-budget">{t("tokenBudgetPerTurn")}</Label>
              <Input
                id="per-turn-budget"
                type="number"
                min={1}
                value={value.tokenBudgetPerTurn}
                onChange={(e) => patch({ tokenBudgetPerTurn: e.target.value })}
                placeholder={t("unlimitedPlaceholder")}
                className="h-8"
                disabled={disabled}
              />
            </div>
          </div>

          <div className="space-y-1.5">
            <Label htmlFor="stall-alert">{t("stallAlertSecs")}</Label>
            <Input
              id="stall-alert"
              type="number"
              min={1}
              value={value.stallAlertSecs}
              onChange={(e) => patch({ stallAlertSecs: e.target.value })}
              placeholder={t("offPlaceholder")}
              className="h-8"
              disabled={disabled}
            />
            <p className="text-xs text-muted-foreground">
              {t("stallAlertHint")}
            </p>
          </div>

          {limitsExtra}
        </TabsContent>
      </div>
    </Tabs>
  )
}

// ─── Per-stage agent editor ──────────────────────────────────────────────────

/** One single-agent stage (or the default): an agent picker plus its live
 *  mode/config. When `allowInherit`, the picker offers "use default" (`INHERIT`)
 *  and selecting it hides the config — the stage follows the default agent. */
function StageAgentEditor({
  spec,
  onChange,
  allowInherit,
  disabled,
}: {
  spec: AgentSpecForm | typeof INHERIT
  onChange: (next: AgentSpecForm | typeof INHERIT) => void
  allowInherit: boolean
  disabled?: boolean
}) {
  const t = useTranslations("Loops.agentConfig")
  const isInherit = spec === INHERIT

  return (
    <div className="space-y-3">
      <Select
        value={isInherit ? INHERIT : spec.agent}
        onValueChange={(v) => {
          if (v === INHERIT) onChange(INHERIT)
          // Switching agent drops any prior mode/config (probed for the old one).
          else
            onChange({
              agent: v as AgentType,
              mode_id: null,
              config_values: {},
            })
        }}
        disabled={disabled}
      >
        <SelectTrigger className="h-8">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          {allowInherit && (
            <SelectItem value={INHERIT}>{t("useDefault")}</SelectItem>
          )}
          {AGENT_TYPES.map((a) => (
            <SelectItem key={a} value={a}>
              {AGENT_LABELS[a]}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>

      {isInherit ? (
        <p className="text-xs text-muted-foreground">{t("useDefaultHint")}</p>
      ) : (
        <AgentConfigBody
          agent={spec.agent}
          modeId={spec.mode_id}
          configValues={spec.config_values}
          onChange={({ mode_id, config_values }) =>
            onChange({ agent: spec.agent, mode_id, config_values })
          }
          disabled={disabled}
        />
      )}
    </div>
  )
}

// ─── Shared agent config body (probe + mode + options) ───────────────────────

interface CachedSnapshot {
  snapshot: AgentOptionsSnapshot
  ts: number
}
const SNAPSHOT_TTL_MS = 30_000
const snapshotCache = new Map<AgentType, CachedSnapshot>()

function readCache(agent: AgentType): AgentOptionsSnapshot | null {
  const entry = snapshotCache.get(agent)
  if (!entry) return null
  if (Date.now() - entry.ts > SNAPSHOT_TTL_MS) {
    snapshotCache.delete(agent)
    return null
  }
  return entry.snapshot
}

/** Live probe of an agent's modes/config options (30s module-scope cache),
 *  mirroring the delegation-settings panel so the config the user picks matches
 *  what the engine will pass when it spawns the agent. */
function useAgentOptions(agent: AgentType): {
  snapshot: AgentOptionsSnapshot | null
  loading: boolean
  error: string | null
} {
  const [snapshot, setSnapshot] = useState<AgentOptionsSnapshot | null>(() =>
    readCache(agent)
  )
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const reqIdRef = useRef(0)

  const load = useCallback(async (a: AgentType) => {
    const cached = readCache(a)
    if (cached) {
      setSnapshot(cached)
      setError(null)
      setLoading(false)
      return
    }
    const reqId = ++reqIdRef.current
    setLoading(true)
    setError(null)
    setSnapshot(null)
    try {
      const fresh = await describeAgentOptions(a)
      if (reqIdRef.current !== reqId) return
      snapshotCache.set(a, { snapshot: fresh, ts: Date.now() })
      setSnapshot(fresh)
    } catch (err) {
      if (reqIdRef.current !== reqId) return
      setError(toErrorMessage(err))
    } finally {
      if (reqIdRef.current === reqId) setLoading(false)
    }
  }, [])

  useEffect(() => {
    void load(agent)
  }, [agent, load])

  return { snapshot, loading, error }
}

/** Probes `agent` and renders its mode row (when standalone) + config option
 *  rows. Reused by each per-stage agent editor and by each reviewer row. */
function AgentConfigBody({
  agent,
  modeId,
  configValues,
  onChange,
  disabled,
}: {
  agent: AgentType
  modeId: string | null
  configValues: Record<string, string>
  onChange: (next: {
    mode_id: string | null
    config_values: Record<string, string>
  }) => void
  disabled?: boolean
}) {
  const t = useTranslations("Loops.reviewers")
  const { snapshot, loading, error } = useAgentOptions(agent)

  const setMode = (modeId: string | null) =>
    onChange({ mode_id: modeId, config_values: configValues })
  const setConfigValue = (optionId: string, valueId: string | null) => {
    const next = { ...configValues }
    if (valueId === null) delete next[optionId]
    else next[optionId] = valueId
    onChange({ mode_id: modeId, config_values: next })
  }

  const hasModes =
    !!snapshot?.modes && snapshot.modes.available_modes.length > 0
  const hasOptions = !!snapshot && snapshot.config_options.length > 0
  // Mirror the chat input / delegation panel: when an agent exposes both modes
  // and config options, the mode is already one of the options — hide the
  // standalone mode row to avoid a duplicate.
  const showStandaloneMode = hasModes && !hasOptions

  return (
    <>
      {loading && (
        <div className="flex items-center gap-2 text-xs text-muted-foreground">
          <Loader2 className="size-3.5 animate-spin" aria-hidden />
          {t("probing")}
        </div>
      )}
      {error && !loading && (
        <p className="text-xs text-muted-foreground">{t("probeFailed")}</p>
      )}
      {!loading && !error && snapshot && (
        <div className="space-y-2">
          {showStandaloneMode && snapshot.modes && (
            <AgentModeRow
              modes={snapshot.modes.available_modes}
              agentDefaultModeId={snapshot.modes.current_mode_id}
              overrideModeId={modeId}
              onChange={setMode}
              disabled={disabled}
            />
          )}
          {snapshot.config_options.map((option) => (
            <AgentConfigRow
              key={option.id}
              option={option}
              overrideValue={configValues[option.id] ?? null}
              onChange={(valueId) => setConfigValue(option.id, valueId)}
              disabled={disabled}
            />
          ))}
          {!showStandaloneMode && !hasOptions && (
            <p className="text-xs text-muted-foreground">{t("noConfig")}</p>
          )}
        </div>
      )}
    </>
  )
}

// ─── Reviewers editor ────────────────────────────────────────────────────────

function ReviewersEditor({
  value,
  onChange,
  disabled,
}: {
  value: ReviewerForm[]
  onChange: (reviewers: ReviewerForm[]) => void
  disabled?: boolean
}) {
  const t = useTranslations("Loops.reviewers")

  const setRow = (i: number, next: ReviewerForm) =>
    onChange(value.map((r, j) => (j === i ? next : r)))
  const addRow = () =>
    onChange([
      ...value,
      { agent: "claude_code", mode_id: null, config_values: {} },
    ])
  const removeRow = (i: number) => onChange(value.filter((_, j) => j !== i))

  return (
    <div className="space-y-2">
      <Label>{t("heading")}</Label>
      <p className="text-xs text-muted-foreground">
        {value.length === 0 ? t("empty") : t("hint")}
      </p>
      <div className="space-y-2">
        {value.map((spec, i) => (
          <ReviewerRow
            key={i}
            index={i}
            spec={spec}
            onChange={(next) => setRow(i, next)}
            onRemove={() => removeRow(i)}
            disabled={disabled}
          />
        ))}
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-8"
          onClick={addRow}
          disabled={disabled}
        >
          <Plus className="mr-1 h-3.5 w-3.5" />
          {t("add")}
        </Button>
      </div>
    </div>
  )
}

function ReviewerRow({
  index,
  spec,
  onChange,
  onRemove,
  disabled,
}: {
  index: number
  spec: ReviewerForm
  onChange: (next: ReviewerForm) => void
  onRemove: () => void
  disabled?: boolean
}) {
  const t = useTranslations("Loops.reviewers")
  const tAgent = useTranslations("Loops.agentConfig")
  const inherit = isInheritReviewer(spec)

  return (
    <div className="space-y-2 rounded-md border bg-card/50 p-2.5">
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-muted-foreground">
          {t("rowLabel", { n: index + 1 })}
        </span>
        <div className="flex-1">
          <Select
            value={inherit ? INHERIT : spec.agent}
            onValueChange={(v) =>
              // Switching agent drops any prior mode/config (they were probed
              // for the old agent and won't apply to the new one); INHERIT defers
              // this reviewer to the issue's default review agent.
              onChange(
                v === INHERIT
                  ? { inherit: true }
                  : { agent: v as AgentType, mode_id: null, config_values: {} }
              )
            }
            disabled={disabled}
          >
            <SelectTrigger className="h-8">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
              <SelectItem value={INHERIT}>{tAgent("useDefault")}</SelectItem>
              {AGENT_TYPES.map((a) => (
                <SelectItem key={a} value={a}>
                  {AGENT_LABELS[a]}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>
        <Button
          type="button"
          variant="ghost"
          size="icon"
          className="h-8 w-8 shrink-0"
          onClick={onRemove}
          disabled={disabled}
          aria-label={t("remove")}
        >
          <X className="h-4 w-4" />
        </Button>
      </div>

      {inherit ? (
        <p className="text-xs text-muted-foreground">{t("useDefaultHint")}</p>
      ) : (
        <AgentConfigBody
          agent={spec.agent}
          modeId={spec.mode_id ?? null}
          configValues={spec.config_values}
          onChange={({ mode_id, config_values }) =>
            onChange({ agent: spec.agent, mode_id, config_values })
          }
          disabled={disabled}
        />
      )}
    </div>
  )
}

function AgentModeRow({
  modes,
  agentDefaultModeId,
  overrideModeId,
  onChange,
  disabled,
}: {
  modes: Array<{ id: string; name: string; description?: string | null }>
  agentDefaultModeId: string
  overrideModeId: string | null
  onChange: (modeId: string | null) => void
  disabled?: boolean
}) {
  const t = useTranslations("Loops.reviewers")
  const agentDefaultName =
    modes.find((m) => m.id === agentDefaultModeId)?.name ?? agentDefaultModeId
  const selectValue = overrideModeId ?? DEFAULT_SENTINEL
  return (
    <div className="flex items-start justify-between gap-3">
      <div className="min-w-0 space-y-0.5">
        <p className="text-sm font-medium">{t("modeLabel")}</p>
        <p className="text-xs text-muted-foreground">
          {t("agentDefaultHint", { value: agentDefaultName })}
        </p>
      </div>
      <Select
        value={selectValue}
        onValueChange={(v) => onChange(v === DEFAULT_SENTINEL ? null : v)}
        disabled={disabled}
      >
        <SelectTrigger size="sm" className="w-44 shrink-0">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={DEFAULT_SENTINEL}>
            {t("defaultOption", { value: agentDefaultName })}
          </SelectItem>
          {modes.map((mode) => (
            <SelectItem key={mode.id} value={mode.id}>
              {mode.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  )
}

function AgentConfigRow({
  option,
  overrideValue,
  onChange,
  disabled,
}: {
  option: SessionConfigOptionInfo
  overrideValue: string | null
  onChange: (valueId: string | null) => void
  disabled?: boolean
}) {
  const t = useTranslations("Loops.reviewers")
  if (option.kind.type !== "select") return null

  const allOptions =
    option.kind.groups.length > 0
      ? option.kind.groups.flatMap((g) => g.options)
      : option.kind.options
  const agentDefault = option.kind.current_value
  const agentDefaultLabel =
    allOptions.find((o) => o.value === agentDefault)?.name ?? agentDefault
  const selectValue = overrideValue ?? DEFAULT_SENTINEL

  return (
    <div className="flex items-start justify-between gap-3">
      <div className="min-w-0 space-y-0.5">
        <p className="text-sm font-medium">{option.name}</p>
        <p className="text-xs text-muted-foreground">
          {t("agentDefaultHint", { value: agentDefaultLabel })}
        </p>
      </div>
      <Select
        value={selectValue}
        onValueChange={(v) => onChange(v === DEFAULT_SENTINEL ? null : v)}
        disabled={disabled}
      >
        <SelectTrigger size="sm" className="w-56 shrink-0">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={DEFAULT_SENTINEL}>
            {t("defaultOption", { value: agentDefaultLabel })}
          </SelectItem>
          {option.kind.groups.length > 0
            ? option.kind.groups.map((group) => (
                <SelectGroup key={group.group}>
                  <SelectLabel>{group.name}</SelectLabel>
                  {group.options.map((item) => (
                    <SelectItem
                      key={`${group.group}-${item.value}`}
                      value={item.value}
                    >
                      {item.name}
                    </SelectItem>
                  ))}
                </SelectGroup>
              ))
            : option.kind.options.map((item) => (
                <SelectItem key={item.value} value={item.value}>
                  {item.name}
                </SelectItem>
              ))}
        </SelectContent>
      </Select>
    </div>
  )
}
