"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { Loader2, SlidersHorizontal } from "lucide-react"
import { toast } from "sonner"
import { useAppWorkspace } from "@/contexts/app-workspace-context"
import { listModelProviders, updateFolderWorkspacePreset } from "@/lib/api"
import {
  AGENT_LABELS,
  ALL_AGENT_TYPES,
  type FolderDetail,
  type ModelProviderInfo,
  type WorkspacePreset,
} from "@/lib/types"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Textarea } from "@/components/ui/textarea"

interface WorkspacePresetDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  folder: FolderDetail | null
}

function parseListInput(value: string): string[] {
  return value
    .split(/\r?\n|,/)
    .map((item) => item.trim())
    .filter(Boolean)
}

function parseEnvOverrides(value: string): Record<string, string> {
  const result: Record<string, string> = {}
  for (const line of value.split(/\r?\n/)) {
    const trimmed = line.trim()
    if (!trimmed) continue
    const separator = trimmed.indexOf("=")
    if (separator <= 0) continue
    const key = trimmed.slice(0, separator).trim()
    const envValue = trimmed.slice(separator + 1).trim()
    if (!key) continue
    result[key] = envValue
  }
  return result
}

function formatEnvOverrides(value: Record<string, string>): string {
  return Object.entries(value)
    .map(([key, entryValue]) => `${key}=${entryValue}`)
    .join("\n")
}

function isPresetEmpty(preset: WorkspacePreset): boolean {
  return (
    !preset.default_agent_type &&
    preset.model_provider_id == null &&
    !preset.approval_policy &&
    preset.skill_ids.length === 0 &&
    preset.mcp_server_ids.length === 0 &&
    Object.keys(preset.env_overrides).length === 0
  )
}

export function WorkspacePresetDialog({
  open,
  onOpenChange,
  folder,
}: WorkspacePresetDialogProps) {
  const { refreshFolder } = useAppWorkspace()
  const [providers, setProviders] = useState<ModelProviderInfo[]>([])
  const [loadingProviders, setLoadingProviders] = useState(false)
  const [saving, setSaving] = useState(false)
  const [defaultAgent, setDefaultAgent] = useState<string>("none")
  const [modelProviderId, setModelProviderId] = useState<string>("none")
  const [approvalPolicy, setApprovalPolicy] = useState("")
  const [skillIds, setSkillIds] = useState("")
  const [mcpServerIds, setMcpServerIds] = useState("")
  const [envOverrides, setEnvOverrides] = useState("")

  const hydrate = useCallback(() => {
    const preset = folder?.workspace_preset
    setDefaultAgent(preset?.default_agent_type ?? "none")
    setModelProviderId(
      preset?.model_provider_id != null
        ? String(preset.model_provider_id)
        : "none"
    )
    setApprovalPolicy(preset?.approval_policy ?? "")
    setSkillIds((preset?.skill_ids ?? []).join("\n"))
    setMcpServerIds((preset?.mcp_server_ids ?? []).join("\n"))
    setEnvOverrides(formatEnvOverrides(preset?.env_overrides ?? {}))
  }, [folder])

  useEffect(() => {
    if (!open) return
    hydrate()
  }, [hydrate, open])

  useEffect(() => {
    if (!open) return
    setLoadingProviders(true)
    listModelProviders()
      .then((items) => setProviders(items))
      .catch((error) => {
        console.error(
          "[WorkspacePresetDialog] load model providers failed:",
          error
        )
      })
      .finally(() => setLoadingProviders(false))
  }, [open])

  const title = useMemo(() => {
    if (!folder) return "Workspace preset"
    return `Workspace preset · ${folder.name}`
  }, [folder])

  const handleSave = useCallback(async () => {
    if (!folder) return

    const preset: WorkspacePreset = {
      default_agent_type:
        defaultAgent === "none"
          ? null
          : (defaultAgent as WorkspacePreset["default_agent_type"]),
      model_provider_id:
        modelProviderId === "none" ? null : Number(modelProviderId),
      approval_policy: approvalPolicy.trim() || null,
      skill_ids: parseListInput(skillIds),
      mcp_server_ids: parseListInput(mcpServerIds),
      env_overrides: parseEnvOverrides(envOverrides),
    }

    setSaving(true)
    try {
      await updateFolderWorkspacePreset(
        folder.id,
        isPresetEmpty(preset) ? null : preset
      )
      await refreshFolder(folder.id)
      toast.success("Workspace preset saved")
      onOpenChange(false)
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error)
      toast.error(`Failed to save preset: ${message}`)
    } finally {
      setSaving(false)
    }
  }, [
    approvalPolicy,
    defaultAgent,
    envOverrides,
    folder,
    mcpServerIds,
    modelProviderId,
    onOpenChange,
    refreshFolder,
    skillIds,
  ])

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-2xl">
        <DialogHeader>
          <DialogTitle className="flex items-center gap-2">
            <SlidersHorizontal className="h-4 w-4" />
            {title}
          </DialogTitle>
        </DialogHeader>

        <div className="grid gap-4 py-2">
          <div className="grid gap-2 sm:grid-cols-2">
            <div className="space-y-2">
              <Label>Default agent</Label>
              <Select value={defaultAgent} onValueChange={setDefaultAgent}>
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">Inherit</SelectItem>
                  {ALL_AGENT_TYPES.map((agentType) => (
                    <SelectItem key={agentType} value={agentType}>
                      {AGENT_LABELS[agentType]}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="space-y-2">
              <Label>Model provider</Label>
              <Select
                value={modelProviderId}
                onValueChange={setModelProviderId}
              >
                <SelectTrigger>
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  <SelectItem value="none">Inherit</SelectItem>
                  {providers.map((provider) => (
                    <SelectItem key={provider.id} value={String(provider.id)}>
                      {provider.name}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
              {loadingProviders && (
                <div className="text-xs text-muted-foreground">
                  Loading providers…
                </div>
              )}
            </div>
          </div>

          <div className="space-y-2">
            <Label>Approval policy</Label>
            <Input
              value={approvalPolicy}
              onChange={(event) => setApprovalPolicy(event.target.value)}
              placeholder="Optional free-form policy label"
            />
          </div>

          <div className="grid gap-4 sm:grid-cols-2">
            <div className="space-y-2">
              <Label>Skills</Label>
              <Textarea
                rows={5}
                value={skillIds}
                onChange={(event) => setSkillIds(event.target.value)}
                placeholder="One skill id per line"
              />
            </div>

            <div className="space-y-2">
              <Label>MCP servers</Label>
              <Textarea
                rows={5}
                value={mcpServerIds}
                onChange={(event) => setMcpServerIds(event.target.value)}
                placeholder="One server id per line"
              />
            </div>
          </div>

          <div className="space-y-2">
            <Label>Environment overrides</Label>
            <Textarea
              rows={6}
              value={envOverrides}
              onChange={(event) => setEnvOverrides(event.target.value)}
              placeholder={"OPENAI_API_BASE=https://...\nOPENAI_API_KEY=..."}
            />
            <p className="text-xs text-muted-foreground">
              Applied to ACP sessions started in this workspace.
            </p>
          </div>
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => onOpenChange(false)}
            disabled={saving}
          >
            Cancel
          </Button>
          <Button onClick={handleSave} disabled={saving || !folder}>
            {saving && <Loader2 className="h-4 w-4 animate-spin" />}
            Save preset
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
