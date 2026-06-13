"use client"

import { useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Loader2, Plus, X } from "lucide-react"

import { updateLoopIssueConfig } from "@/lib/loops-api"
import { toErrorMessage } from "@/lib/app-error"
import {
  AGENT_LABELS,
  type AgentType,
  type IssueConfig,
  type LoopIssueDetail,
  type LoopIssueRoute,
  type LoopStage,
} from "@/lib/types"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Switch } from "@/components/ui/switch"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

const AGENT_TYPES: AgentType[] = [
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

interface FormState {
  defaultAgent: AgentType
  stageAgents: Record<string, AgentType | typeof INHERIT>
  validationCommands: string[]
  reviewerCount: string
  reviewPassRule: string
  maxAttempts: string
  autoMerge: boolean
  forceRoute: string
  iterationTimeoutSecs: string
  tokenBudgetPerTurn: string
  tokenBudget: string
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

function initForm(issue: LoopIssueDetail): FormState {
  const c = issue.config
  const stageAgents: Record<string, AgentType | typeof INHERIT> = {}
  for (const s of STAGES) stageAgents[s] = c.agents[s] ?? INHERIT
  const route = c.force_route
  return {
    defaultAgent: c.agents.default ?? "claude_code",
    stageAgents,
    validationCommands: [...c.validation_commands],
    reviewerCount: String(c.reviewer_count ?? 1),
    reviewPassRule: c.review_pass_rule || "unanimous",
    maxAttempts: String(c.max_attempts ?? 0),
    autoMerge: !!c.auto_merge,
    forceRoute: route && route !== "undecided" ? route : ROUTE_AUTO,
    iterationTimeoutSecs: intField(c.iteration_timeout_secs),
    tokenBudgetPerTurn: intField(c.token_budget_per_turn),
    tokenBudget: intField(issue.token_budget),
  }
}

function buildConfig(issue: LoopIssueDetail, form: FormState): IssueConfig {
  const agents: Record<string, AgentType> = { default: form.defaultAgent }
  for (const s of STAGES) {
    const v = form.stageAgents[s]
    if (v !== INHERIT) agents[s] = v
  }
  return {
    v: issue.config.v ?? 1,
    agents,
    validation_commands: form.validationCommands
      .map((s) => s.trim())
      .filter(Boolean),
    reviewer_count: parseCount(form.reviewerCount, 1, 1),
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
  }
}

/**
 * Editor for a single issue's `IssueConfig` (per-stage agents, validation
 * commands, reviewer policy, breakers, merge mode, route override, budgets) plus
 * its total token budget. Saving persists via `update_loop_issue_config`, which
 * emits `loop://changed` so the detail view refreshes. The engine reads config
 * fresh each dispatch, so edits to a running issue take effect from its next
 * iteration — surfaced as a hint rather than blocked.
 */
export function IssueSettingsDialog({
  open,
  onOpenChange,
  issue,
}: {
  open: boolean
  onOpenChange: (open: boolean) => void
  issue: LoopIssueDetail
}) {
  const t = useTranslations("Loops.issueSettings")
  const tStage = useTranslations("Loops.stage")
  const tRoute = useTranslations("Loops.route")
  const tCommon = useTranslations("Loops.common")
  const tToasts = useTranslations("Loops.toasts")

  const [form, setForm] = useState<FormState>(() => initForm(issue))
  const [saving, setSaving] = useState(false)

  // Re-seed the form from the issue each time the dialog opens, so a cancel +
  // reopen discards unsaved edits and a config change elsewhere is reflected.
  useEffect(() => {
    if (open) setForm(initForm(issue))
  }, [open, issue])

  const patch = (p: Partial<FormState>) => setForm((f) => ({ ...f, ...p }))

  const setCommand = (i: number, value: string) =>
    setForm((f) => {
      const next = [...f.validationCommands]
      next[i] = value
      return { ...f, validationCommands: next }
    })
  const addCommand = () =>
    setForm((f) => ({
      ...f,
      validationCommands: [...f.validationCommands, ""],
    }))
  const removeCommand = (i: number) =>
    setForm((f) => ({
      ...f,
      validationCommands: f.validationCommands.filter((_, j) => j !== i),
    }))

  const onSave = async () => {
    setSaving(true)
    try {
      await updateLoopIssueConfig(
        issue.id,
        buildConfig(issue, form),
        parsePositiveOrNull(form.tokenBudget)
      )
      toast.success(tToasts("configSaved"))
      onOpenChange(false)
    } catch (err) {
      toast.error(tToasts("actionFailed", { message: toErrorMessage(err) }))
    } finally {
      setSaving(false)
    }
  }

  const agentSelect = (
    value: string,
    onChange: (v: string) => void,
    withInherit: boolean
  ) => (
    <Select value={value} onValueChange={onChange}>
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
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("title")}</DialogTitle>
          <DialogDescription>{t("description")}</DialogDescription>
        </DialogHeader>

        {issue.status === "running" && (
          <p className="rounded-md bg-muted px-3 py-2 text-xs text-muted-foreground">
            {t("runningHint")}
          </p>
        )}

        <div className="max-h-[60vh] space-y-5 overflow-y-auto pr-1">
          {/* Agents */}
          <div className="space-y-2">
            <Label htmlFor="default-agent">{t("defaultAgent")}</Label>
            <div id="default-agent">
              {agentSelect(
                form.defaultAgent,
                (v) => patch({ defaultAgent: v as AgentType }),
                false
              )}
            </div>
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
                    form.stageAgents[s],
                    (v) =>
                      setForm((f) => ({
                        ...f,
                        stageAgents: {
                          ...f.stageAgents,
                          [s]: v as AgentType | typeof INHERIT,
                        },
                      })),
                    true
                  )}
                </div>
              ))}
            </div>
          </div>

          {/* Validation commands */}
          <div className="space-y-2">
            <Label>{t("validationCommands")}</Label>
            <p className="text-xs text-muted-foreground">
              {t("validationHint")}
            </p>
            <div className="space-y-2">
              {form.validationCommands.map((cmd, i) => (
                <div key={i} className="flex gap-2">
                  <Input
                    value={cmd}
                    onChange={(e) => setCommand(i, e.target.value)}
                    placeholder={t("commandPlaceholder")}
                    className="h-8 font-mono text-xs"
                  />
                  <Button
                    type="button"
                    variant="ghost"
                    size="icon"
                    className="h-8 w-8 shrink-0"
                    onClick={() => removeCommand(i)}
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
              >
                <Plus className="mr-1 h-3.5 w-3.5" />
                {t("addCommand")}
              </Button>
            </div>
          </div>

          {/* Review policy */}
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="reviewer-count">{t("reviewerCount")}</Label>
              <Input
                id="reviewer-count"
                type="number"
                min={1}
                value={form.reviewerCount}
                onChange={(e) => patch({ reviewerCount: e.target.value })}
                className="h-8"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="pass-rule">{t("reviewPassRule")}</Label>
              <div id="pass-rule">
                <Select
                  value={form.reviewPassRule}
                  onValueChange={(v) => patch({ reviewPassRule: v })}
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
          </div>

          {/* Breakers + route */}
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="max-attempts">{t("maxAttempts")}</Label>
              <Input
                id="max-attempts"
                type="number"
                min={0}
                value={form.maxAttempts}
                onChange={(e) => patch({ maxAttempts: e.target.value })}
                className="h-8"
              />
              <p className="text-xs text-muted-foreground">
                {t("maxAttemptsHint")}
              </p>
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="force-route">{t("forceRoute")}</Label>
              <div id="force-route">
                <Select
                  value={form.forceRoute}
                  onValueChange={(v) => patch({ forceRoute: v })}
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

          {/* Auto-merge */}
          <div className="flex items-center justify-between gap-3">
            <div>
              <Label htmlFor="auto-merge">{t("autoMerge")}</Label>
              <p className="text-xs text-muted-foreground">
                {t("autoMergeHint")}
              </p>
            </div>
            <Switch
              id="auto-merge"
              checked={form.autoMerge}
              onCheckedChange={(v) => patch({ autoMerge: v })}
            />
          </div>

          {/* Budgets + timeout */}
          <div className="grid grid-cols-2 gap-3">
            <div className="space-y-1.5">
              <Label htmlFor="iter-timeout">{t("iterationTimeout")}</Label>
              <Input
                id="iter-timeout"
                type="number"
                min={1}
                value={form.iterationTimeoutSecs}
                onChange={(e) =>
                  patch({ iterationTimeoutSecs: e.target.value })
                }
                placeholder={t("unlimitedPlaceholder")}
                className="h-8"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="per-turn-budget">{t("tokenBudgetPerTurn")}</Label>
              <Input
                id="per-turn-budget"
                type="number"
                min={1}
                value={form.tokenBudgetPerTurn}
                onChange={(e) => patch({ tokenBudgetPerTurn: e.target.value })}
                placeholder={t("unlimitedPlaceholder")}
                className="h-8"
              />
            </div>
            <div className="space-y-1.5">
              <Label htmlFor="total-budget">{t("tokenBudget")}</Label>
              <Input
                id="total-budget"
                type="number"
                min={1}
                value={form.tokenBudget}
                onChange={(e) => patch({ tokenBudget: e.target.value })}
                placeholder={t("unlimitedPlaceholder")}
                className="h-8"
              />
            </div>
          </div>
        </div>

        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={saving}
          >
            {tCommon("cancel")}
          </Button>
          <Button type="button" onClick={onSave} disabled={saving}>
            {saving && <Loader2 className="mr-2 h-4 w-4 animate-spin" />}
            {t("save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
