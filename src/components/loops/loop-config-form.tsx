"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2, Plus, X } from "lucide-react"

import { describeAgentOptions } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import {
  AGENT_LABELS,
  type AgentOptionsSnapshot,
  type AgentType,
  type IssueConfig,
  type LoopIssueRoute,
  type LoopStage,
  type ReviewerSpec,
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

const STAGES: LoopStage[] = [
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
// Reviewer mode/config "use the agent's own default" (clears the override).
const DEFAULT_SENTINEL = "__codeg_default__"

/**
 * Form-state mirror of `IssueConfig`. Numeric fields are kept as strings so a
 * field can be cleared / typed through intermediate values without snapping to
 * a parsed number; `formStateToConfig` serializes on save. Reviewers are kept
 * in their `ReviewerSpec` shape (already structured, no string mirror needed).
 * The per-issue total token budget is NOT here — it lives outside `IssueConfig`,
 * owned by the issue-settings host.
 */
export interface LoopConfigFormState {
  configVersion: number
  defaultAgent: AgentType
  stageAgents: Record<string, AgentType | typeof INHERIT>
  validationCommands: string[]
  reviewers: ReviewerSpec[]
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

export function configToFormState(c: IssueConfig): LoopConfigFormState {
  const stageAgents: Record<string, AgentType | typeof INHERIT> = {}
  for (const s of STAGES) stageAgents[s] = c.agents[s] ?? INHERIT
  const route = c.force_route
  return {
    configVersion: c.v ?? 1,
    defaultAgent: c.agents.default ?? "claude_code",
    stageAgents,
    validationCommands: [...c.validation_commands],
    reviewers: (c.reviewers ?? []).map((r) => ({
      agent: r.agent,
      mode_id: r.mode_id ?? null,
      config_values: { ...r.config_values },
    })),
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
  const agents: Record<string, AgentType> = { default: form.defaultAgent }
  for (const s of STAGES) {
    const v = form.stageAgents[s]
    if (v !== INHERIT) agents[s] = v
  }
  const reviewers: ReviewerSpec[] = form.reviewers.map((r) => ({
    agent: r.agent,
    ...(r.mode_id ? { mode_id: r.mode_id } : {}),
    config_values: r.config_values,
  }))
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
 * Tabbed editor for an `IssueConfig` (per-stage agents, reviewers, validation
 * commands, breakers, merge mode, route override, budgets). Controlled — the
 * host owns the `LoopConfigFormState` and re-seeds it (e.g. on dialog open).
 * Shared by the per-issue settings dialog and the space-defaults dialog.
 */
export function LoopConfigForm({
  value,
  onChange,
  disabled,
}: {
  value: LoopConfigFormState
  onChange: (next: LoopConfigFormState) => void
  disabled?: boolean
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

  const agentSelect = (
    val: string,
    onChangeVal: (v: string) => void,
    withInherit: boolean
  ) => (
    <Select value={val} onValueChange={onChangeVal} disabled={disabled}>
      <SelectTrigger className="h-8">
        <SelectValue />
      </SelectTrigger>
      <SelectContent>
        {withInherit && <SelectItem value={INHERIT}>{t("inherit")}</SelectItem>}
        {AGENT_TYPES.map((a) => (
          <SelectItem key={a} value={a}>
            {AGENT_LABELS[a]}
          </SelectItem>
        ))}
      </SelectContent>
    </Select>
  )

  return (
    <Tabs defaultValue="agents" className="flex flex-col">
      <TabsList className="self-start">
        <TabsTrigger value="agents">{tCfg("tabAgents")}</TabsTrigger>
        <TabsTrigger value="review">{tCfg("tabReview")}</TabsTrigger>
        <TabsTrigger value="validation">{tCfg("tabValidation")}</TabsTrigger>
        <TabsTrigger value="limits">{tCfg("tabLimits")}</TabsTrigger>
      </TabsList>

      <div className="mt-3 max-h-[52vh] overflow-y-auto pr-1">
        {/* Agents */}
        <TabsContent
          value="agents"
          className="space-y-3 data-[state=inactive]:hidden"
        >
          <div className="space-y-2">
            <Label htmlFor="default-agent">{t("defaultAgent")}</Label>
            <div id="default-agent">
              {agentSelect(
                value.defaultAgent,
                (v) => patch({ defaultAgent: v as AgentType }),
                false
              )}
            </div>
          </div>
          <div className="space-y-2">
            <p className="text-xs font-medium text-muted-foreground">
              {t("stageAgents")}
            </p>
            <div className="grid grid-cols-2 gap-2">
              {STAGES.map((s) => (
                <div key={s} className="space-y-1">
                  <span className="text-xs text-muted-foreground">
                    {tStage(s)}
                  </span>
                  {agentSelect(
                    value.stageAgents[s],
                    (v) =>
                      patch({
                        stageAgents: {
                          ...value.stageAgents,
                          [s]: v as AgentType | typeof INHERIT,
                        },
                      }),
                    true
                  )}
                </div>
              ))}
            </div>
          </div>
        </TabsContent>

        {/* Review */}
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
                  <SelectItem value="majority">{t("ruleMajority")}</SelectItem>
                </SelectContent>
              </Select>
            </div>
          </div>
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
        </TabsContent>
      </div>
    </Tabs>
  )
}

// ─── Reviewers editor ────────────────────────────────────────────────────────

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
 *  mirroring the delegation-settings panel so the reviewer config the user
 *  picks matches what the engine will pass when it spawns the reviewer. */
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

function ReviewersEditor({
  value,
  onChange,
  disabled,
}: {
  value: ReviewerSpec[]
  onChange: (reviewers: ReviewerSpec[]) => void
  disabled?: boolean
}) {
  const t = useTranslations("Loops.reviewers")

  const setRow = (i: number, next: ReviewerSpec) =>
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
  spec: ReviewerSpec
  onChange: (next: ReviewerSpec) => void
  onRemove: () => void
  disabled?: boolean
}) {
  const t = useTranslations("Loops.reviewers")
  const { snapshot, loading, error } = useAgentOptions(spec.agent)

  const setMode = (modeId: string | null) =>
    onChange({ ...spec, mode_id: modeId })
  const setConfigValue = (optionId: string, valueId: string | null) => {
    const config_values = { ...spec.config_values }
    if (valueId === null) delete config_values[optionId]
    else config_values[optionId] = valueId
    onChange({ ...spec, config_values })
  }

  const hasModes =
    !!snapshot?.modes && snapshot.modes.available_modes.length > 0
  const hasOptions = !!snapshot && snapshot.config_options.length > 0
  // Mirror the chat input / delegation panel: when an agent exposes both modes
  // and config options, the mode is already one of the options — hide the
  // standalone mode row to avoid a duplicate.
  const showStandaloneMode = hasModes && !hasOptions

  return (
    <div className="space-y-2 rounded-md border bg-card/50 p-2.5">
      <div className="flex items-center gap-2">
        <span className="text-xs font-medium text-muted-foreground">
          {t("rowLabel", { n: index + 1 })}
        </span>
        <div className="flex-1">
          <Select
            value={spec.agent}
            onValueChange={(v) =>
              // Switching agent drops any prior mode/config (they were probed
              // for the old agent and won't apply to the new one).
              onChange({
                agent: v as AgentType,
                mode_id: null,
                config_values: {},
              })
            }
            disabled={disabled}
          >
            <SelectTrigger className="h-8">
              <SelectValue />
            </SelectTrigger>
            <SelectContent>
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
            <ReviewerModeRow
              modes={snapshot.modes.available_modes}
              agentDefaultModeId={snapshot.modes.current_mode_id}
              overrideModeId={spec.mode_id ?? null}
              onChange={setMode}
              disabled={disabled}
            />
          )}
          {snapshot.config_options.map((option) => (
            <ReviewerConfigRow
              key={option.id}
              option={option}
              overrideValue={spec.config_values[option.id] ?? null}
              onChange={(valueId) => setConfigValue(option.id, valueId)}
              disabled={disabled}
            />
          ))}
          {!showStandaloneMode && !hasOptions && (
            <p className="text-xs text-muted-foreground">{t("noConfig")}</p>
          )}
        </div>
      )}
    </div>
  )
}

function ReviewerModeRow({
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
    <div className="flex items-center justify-between gap-2">
      <span className="text-xs text-muted-foreground">{t("modeLabel")}</span>
      <Select
        value={selectValue}
        onValueChange={(v) => onChange(v === DEFAULT_SENTINEL ? null : v)}
        disabled={disabled}
      >
        <SelectTrigger className="h-8 w-44">
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

function ReviewerConfigRow({
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
    <div className="flex items-center justify-between gap-2">
      <span className="min-w-0 truncate text-xs text-muted-foreground">
        {option.name}
      </span>
      <Select
        value={selectValue}
        onValueChange={(v) => onChange(v === DEFAULT_SENTINEL ? null : v)}
        disabled={disabled}
      >
        <SelectTrigger className="h-8 w-44">
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
