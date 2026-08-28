"use client"

import { useEffect, useState } from "react"
import { Plus } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"
import { acpListAgents, acpUpdateAgentConfig } from "@/lib/api"
import { isKiloAgentType, type AgentType } from "@/lib/types"
import {
  addKiloModel,
  kiloModelsFromConfig,
  setKiloModelReasoning,
  setKiloModelVariantEnabled,
  type KiloModelEntry,
} from "@/lib/kilo-model-config"

interface KiloChatModelManagerProps {
  open: boolean
  onOpenChange: (open: boolean) => void
}

const KILO_AGENT_TYPE: AgentType = "custom:kilo"

export function KiloChatModelManager({
  open,
  onOpenChange,
}: KiloChatModelManagerProps) {
  const t = useTranslations("Folder.chat.kiloModelManager")
  const [config, setConfig] = useState<Record<string, unknown>>({})
  const [providerId, setProviderId] = useState("")
  const [modelId, setModelId] = useState("")
  const [saving, setSaving] = useState(false)

  useEffect(() => {
    if (!open) return
    acpListAgents()
      .then((agents) => {
        const raw = agents.find((agent) =>
          isKiloAgentType(agent.agent_type)
        )?.config_json
        setConfig(raw ? JSON.parse(raw) : {})
      })
      .catch((error) => {
        toast.error(t("loadFailed"), {
          description: error instanceof Error ? error.message : String(error),
        })
      })
  }, [open, t])

  const save = async (next: Record<string, unknown>) => {
    setSaving(true)
    try {
      await acpUpdateAgentConfig(KILO_AGENT_TYPE, {
        config_json: JSON.stringify(next, null, 2),
      })
      setConfig(next)
    } catch (error) {
      toast.error(t("saveFailed"), {
        description: error instanceof Error ? error.message : String(error),
      })
    } finally {
      setSaving(false)
    }
  }

  const addModel = () => {
    const provider = providerId.trim()
    const model = modelId.trim()
    if (!provider || !model) return
    void save(addKiloModel(config, provider, model))
    setModelId("")
  }

  const setReasoning = (entry: KiloModelEntry, reasoning: boolean) => {
    void save(setKiloModelReasoning(config, entry, reasoning))
  }

  const models = kiloModelsFromConfig(config)

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("title")}</DialogTitle>
          <DialogDescription>{t("description")}</DialogDescription>
        </DialogHeader>
        <div className="grid gap-2 sm:grid-cols-[1fr_1fr_auto]">
          <Input
            value={providerId}
            onChange={(event) => setProviderId(event.target.value)}
            placeholder={t("providerPlaceholder")}
          />
          <Input
            value={modelId}
            onChange={(event) => setModelId(event.target.value)}
            placeholder={t("modelPlaceholder")}
          />
          <Button
            type="button"
            onClick={addModel}
            disabled={saving || !providerId.trim() || !modelId.trim()}
          >
            <Plus className="size-3.5" /> {t("add")}
          </Button>
        </div>
        <div className="max-h-72 space-y-1 overflow-y-auto rounded-md border p-1">
          {models.map((entry) => (
            <div
              key={`${entry.providerId}/${entry.modelId}`}
              className="flex items-center gap-2 rounded px-2 py-1.5 text-sm"
            >
              <span className="min-w-0 flex-1 truncate font-mono text-xs">
                {entry.providerId}/{entry.modelId}
              </span>
              <span className="text-xs text-muted-foreground">
                {t("reasoning")}
              </span>
              <Switch
                checked={entry.reasoning}
                disabled={saving}
                onCheckedChange={(value) => setReasoning(entry, value)}
              />
            </div>
          ))}
          {models.length === 0 && (
            <p className="px-2 py-4 text-center text-sm text-muted-foreground">
              {t("empty")}
            </p>
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}

export function KiloReasoningVariantsButton({
  modelValue,
}: {
  modelValue: string
}) {
  const t = useTranslations("Folder.chat.kiloModelManager")
  const [open, setOpen] = useState(false)
  const [config, setConfig] = useState<Record<string, unknown>>({})
  const [customLevel, setCustomLevel] = useState("")
  const [saving, setSaving] = useState(false)
  const [providerId, modelId] = modelValue.split(/\/(.*)/)
  const load = () =>
    acpListAgents()
      .then((agents) => {
        const raw = agents.find((agent) =>
          isKiloAgentType(agent.agent_type)
        )?.config_json
        setConfig(raw ? JSON.parse(raw) : {})
      })
      .catch((error) => {
        toast.error(t("loadFailed"), {
          description: error instanceof Error ? error.message : String(error),
        })
      })
  const update = async (level: string, enabled: boolean) => {
    if (!providerId || !modelId) return
    const next = setKiloModelVariantEnabled(
      config,
      providerId,
      modelId,
      level,
      enabled
    )
    setSaving(true)
    try {
      await acpUpdateAgentConfig(KILO_AGENT_TYPE, {
        config_json: JSON.stringify(next, null, 2),
      })
      setConfig(next)
    } catch (error) {
      toast.error(t("saveFailed"), {
        description: error instanceof Error ? error.message : String(error),
      })
    } finally {
      setSaving(false)
    }
  }
  const model = (
    (
      (config.provider as Record<string, unknown>)?.[providerId] as Record<
        string,
        unknown
      >
    )?.models as Record<string, unknown>
  )?.[modelId] as Record<string, unknown> | undefined
  const variants =
    (model?.variants as Record<string, Record<string, unknown>> | undefined) ??
    {}
  const levels = Array.from(
    new Set(["low", "high", "max", ...Object.keys(variants)])
  )
  return (
    <Dialog
      open={open}
      onOpenChange={(value) => {
        setOpen(value)
        if (value) void load()
      }}
    >
      <Button
        type="button"
        variant="ghost"
        size="icon-xs"
        title={t("manageReasoningLevels")}
        aria-label={t("manageReasoningLevels")}
        onClick={() => setOpen(true)}
      >
        <Plus />
      </Button>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>{t("reasoningLevelsTitle")}</DialogTitle>
          <DialogDescription>
            {t("reasoningLevelsDescription", { model: modelValue })}
          </DialogDescription>
        </DialogHeader>
        <div className="space-y-1">
          {levels.map((level) => (
            <div
              key={level}
              className="flex items-center justify-between rounded px-2 py-1.5 text-sm"
            >
              <span>{level}</span>
              <Switch
                checked={variants[level]?.disabled !== true}
                disabled={saving}
                onCheckedChange={(enabled) => void update(level, enabled)}
              />
            </div>
          ))}
        </div>
        <div className="flex gap-2">
          <Input
            value={customLevel}
            onChange={(event) => setCustomLevel(event.target.value)}
            placeholder={t("customLevel")}
          />
          <Button
            type="button"
            size="sm"
            disabled={!customLevel.trim() || saving}
            onClick={() => {
              void update(customLevel.trim(), true)
              setCustomLevel("")
            }}
          >
            {t("add")}
          </Button>
        </div>
      </DialogContent>
    </Dialog>
  )
}
