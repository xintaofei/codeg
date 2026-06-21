"use client"

import { useEffect, useState } from "react"
import { ArrowLeft } from "lucide-react"
import { useTranslations } from "next-intl"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { AgentSelector } from "@/components/chat/agent-selector"
import { AgentConfigSection } from "./agent-config-section"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Textarea } from "@/components/ui/textarea"
import { Switch } from "@/components/ui/switch"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { automationComputeNextRun } from "@/lib/api"
import type {
  AgentType,
  Automation,
  AutomationDraft,
  AutomationIsolation,
  AutomationTriggerKind,
} from "@/lib/types"

interface AutomationEditorProps {
  /** The automation being edited, a template-seeded draft, or `null` for a
   *  blank create. Every field the editor reads is shared by `Automation` and
   *  `AutomationDraft`, so the `??` init chains seed from either. */
  automation: Automation | AutomationDraft | null
  onSubmit: (draft: AutomationDraft) => Promise<void>
  onCancel: () => void
  /** When present (the create-from-gallery flow), renders a "← Templates" link
   *  back to the picker. */
  onBackToTemplates?: () => void
}

const CRON_PRESETS = [
  { key: "presetHourly" as const, cron: "0 * * * *" },
  { key: "presetDaily" as const, cron: "0 9 * * *" },
  { key: "presetWeekdays" as const, cron: "0 9 * * 1-5" },
]

function detectTimezone(): string {
  try {
    return Intl.DateTimeFormat().resolvedOptions().timeZone || "UTC"
  } catch {
    return "UTC"
  }
}

export function AutomationEditor({
  automation,
  onSubmit,
  onCancel,
  onBackToTemplates,
}: AutomationEditorProps) {
  const t = useTranslations("Automations")
  const { folders } = useAppWorkspace()

  const [name, setName] = useState(automation?.name ?? "")
  const [agentType, setAgentType] = useState<AgentType>(
    automation?.agent_type ?? "claude_code"
  )
  const [prompt, setPrompt] = useState(automation?.config?.display_text ?? "")
  const [folderId, setFolderId] = useState<number | null>(
    automation?.root_folder_id ?? folders[0]?.id ?? null
  )
  const [isolation, setIsolation] = useState<AutomationIsolation>(
    automation?.isolation ?? "worktree_per_run"
  )
  const [trigger, setTrigger] = useState<AutomationTriggerKind>(
    automation?.trigger_kind ?? "schedule"
  )
  const [cron, setCron] = useState(automation?.cron ?? "0 9 * * 1-5")
  const [timezone, setTimezone] = useState(
    automation?.timezone ?? detectTimezone()
  )
  const [enabled, setEnabled] = useState(automation?.enabled ?? true)
  const [modeId, setModeId] = useState<string | null>(
    automation?.config?.mode_id ?? null
  )
  const [configValues, setConfigValues] = useState<Record<string, string>>(
    automation?.config?.config_values ?? {}
  )
  const [branch, setBranch] = useState(automation?.branch ?? "")
  const [error, setError] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [nextRun, setNextRun] = useState<string | null>(null)

  // Authoritative "next run" preview — same backend evaluator the scheduler
  // uses, so the previewed time can never diverge from the actual fire.
  useEffect(() => {
    if (trigger !== "schedule" || !cron.trim()) {
      setNextRun(null)
      return
    }
    let cancelled = false
    const handle = setTimeout(() => {
      automationComputeNextRun(cron.trim(), timezone)
        .then((r) => {
          if (!cancelled) setNextRun(r)
        })
        .catch(() => {
          if (!cancelled) setNextRun(null)
        })
    }, 300)
    return () => {
      cancelled = true
      clearTimeout(handle)
    }
  }, [cron, timezone, trigger])

  // Backfill the default folder once the workspace folders finish hydrating — a
  // new (or template-seeded) automation opened before they load would otherwise
  // keep folderId null and block submit on errorFolder. Guarding on
  // `automation?.root_folder_id == null` (rather than `!automation`) also covers
  // a template draft seeded with a null folder, while never overriding the
  // folder of an existing automation being edited (its folderId is non-null, so
  // the `folderId == null` guard already short-circuits).
  useEffect(() => {
    if (
      folderId == null &&
      automation?.root_folder_id == null &&
      folders.length > 0
    ) {
      setFolderId(folders[0].id)
    }
  }, [folders, folderId, automation])

  const submit = async () => {
    setError(null)
    if (!name.trim()) return setError(t("errorName"))
    if (!prompt.trim()) return setError(t("errorPrompt"))
    if (trigger === "schedule" && !cron.trim()) return setError(t("errorCron"))
    if (folderId == null) return setError(t("errorFolder"))

    const draft: AutomationDraft = {
      name: name.trim(),
      enabled,
      trigger_kind: trigger,
      cron: trigger === "schedule" ? cron.trim() : null,
      timezone,
      agent_type: agentType,
      root_folder_id: folderId,
      isolation,
      branch:
        isolation === "shared_in_root" && branch.trim() ? branch.trim() : null,
      is_remote_branch: false,
      config: {
        prompt_blocks: [{ type: "text", text: prompt.trim() }],
        display_text: prompt.trim(),
        mode_id: modeId,
        config_values: configValues,
      },
    }
    setSaving(true)
    try {
      await onSubmit(draft)
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto px-1 py-1">
      {onBackToTemplates ? (
        <button
          type="button"
          onClick={onBackToTemplates}
          className="-ml-1 inline-flex w-fit items-center gap-1 text-xs text-muted-foreground transition-colors hover:text-foreground"
        >
          <ArrowLeft className="h-3.5 w-3.5" aria-hidden="true" />
          {t("backToTemplates")}
        </button>
      ) : null}

      <div className="flex flex-col gap-1.5">
        <Label htmlFor="automation-name">{t("name")}</Label>
        <Input
          id="automation-name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          placeholder={t("namePlaceholder")}
        />
      </div>

      <div className="flex flex-col gap-1.5">
        <Label>{t("agent")}</Label>
        <AgentSelector
          defaultAgentType={agentType}
          onSelect={(a) => {
            // Switching agents changes the option universe — reset overrides.
            setAgentType(a)
            setModeId(null)
            setConfigValues({})
          }}
          // A system substitution (saved agent unavailable) updates the type but
          // must NOT be treated as a user choice that wipes the saved config.
          onFallback={setAgentType}
        />
      </div>

      <div className="flex flex-col gap-1.5">
        <Label>{t("config")}</Label>
        <AgentConfigSection
          agentType={agentType}
          modeId={modeId}
          configValues={configValues}
          onModeChange={setModeId}
          onConfigChange={(optionId, valueId) =>
            setConfigValues((prev) => {
              const next = { ...prev }
              if (valueId === null) delete next[optionId]
              else next[optionId] = valueId
              return next
            })
          }
        />
      </div>

      <div className="flex flex-col gap-1.5">
        <Label htmlFor="automation-prompt">{t("prompt")}</Label>
        <Textarea
          id="automation-prompt"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          placeholder={t("promptPlaceholder")}
          rows={4}
        />
      </div>

      <div className="flex flex-col gap-1.5">
        <Label>{t("folder")}</Label>
        <Select
          value={folderId != null ? String(folderId) : undefined}
          onValueChange={(v) => setFolderId(Number(v))}
        >
          <SelectTrigger>
            <SelectValue placeholder={t("folderPlaceholder")} />
          </SelectTrigger>
          <SelectContent>
            {folders.map((f) => (
              <SelectItem key={f.id} value={String(f.id)}>
                {f.name}
              </SelectItem>
            ))}
          </SelectContent>
        </Select>
      </div>

      <div className="flex flex-col gap-1.5">
        <Label>{t("isolation")}</Label>
        <RadioGroup
          value={isolation}
          onValueChange={(v) => setIsolation(v as AutomationIsolation)}
          className="flex flex-col gap-2"
        >
          <label className="flex items-center gap-2 text-sm">
            <RadioGroupItem value="worktree_per_run" />
            {t("isolationWorktree")}
          </label>
          <label className="flex items-center gap-2 text-sm">
            <RadioGroupItem value="shared_in_root" />
            {t("isolationShared")}
          </label>
        </RadioGroup>
      </div>

      {isolation === "shared_in_root" ? (
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="automation-branch">{t("branch")}</Label>
          <Input
            id="automation-branch"
            value={branch}
            onChange={(e) => setBranch(e.target.value)}
            placeholder={t("branchPlaceholder")}
            className="font-mono"
          />
        </div>
      ) : null}

      <div className="flex flex-col gap-1.5">
        <Label>{t("trigger")}</Label>
        <RadioGroup
          value={trigger}
          onValueChange={(v) => setTrigger(v as AutomationTriggerKind)}
          className="flex flex-col gap-2"
        >
          <label className="flex items-center gap-2 text-sm">
            <RadioGroupItem value="schedule" />
            {t("triggerSchedule")}
          </label>
          <label className="flex items-center gap-2 text-sm">
            <RadioGroupItem value="manual" />
            {t("triggerManual")}
          </label>
        </RadioGroup>
      </div>

      {trigger === "schedule" ? (
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="automation-cron">{t("cron")}</Label>
          <div className="flex flex-wrap gap-1.5">
            {CRON_PRESETS.map((p) => (
              <Button
                key={p.key}
                type="button"
                size="sm"
                variant={cron === p.cron ? "default" : "outline"}
                onClick={() => setCron(p.cron)}
              >
                {t(p.key)}
              </Button>
            ))}
          </div>
          <Input
            id="automation-cron"
            value={cron}
            onChange={(e) => setCron(e.target.value)}
            placeholder={t("cronPlaceholder")}
            className="font-mono"
          />
          <Input
            value={timezone}
            onChange={(e) => setTimezone(e.target.value)}
            placeholder={t("timezone")}
            aria-label={t("timezone")}
            className="font-mono"
          />
          <p className="text-xs text-muted-foreground">
            {t("nextRun")}: {nextRun ? new Date(nextRun).toLocaleString() : "—"}
          </p>
        </div>
      ) : null}

      <label className="flex items-center gap-2 text-sm">
        <Switch checked={enabled} onCheckedChange={setEnabled} />
        {t("enabled")}
      </label>

      {error ? (
        <p className="text-sm text-destructive" role="alert">
          {error}
        </p>
      ) : null}

      <div className="mt-1 flex justify-end gap-2">
        <Button
          type="button"
          variant="ghost"
          onClick={onCancel}
          disabled={saving}
        >
          {t("cancel")}
        </Button>
        <Button type="button" onClick={submit} disabled={saving}>
          {t("save")}
        </Button>
      </div>
    </div>
  )
}
