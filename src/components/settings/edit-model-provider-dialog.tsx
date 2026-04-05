"use client"

import { useCallback, useEffect, useState } from "react"
import { Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  Dialog,
  DialogContent,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { updateModelProvider } from "@/lib/api"
import {
  ALL_AGENT_TYPES,
  AGENT_LABELS,
  type AgentType,
  type ModelProviderInfo,
} from "@/lib/types"

interface EditModelProviderDialogProps {
  provider: ModelProviderInfo | null
  onOpenChange: (open: boolean) => void
  onProviderUpdated: () => void
}

export function EditModelProviderDialog({
  provider,
  onOpenChange,
  onProviderUpdated,
}: EditModelProviderDialogProps) {
  const t = useTranslations("ModelProviderSettings")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [name, setName] = useState("")
  const [apiUrl, setApiUrl] = useState("")
  const [apiKey, setApiKey] = useState("")
  const [selectedTypes, setSelectedTypes] = useState<AgentType[]>([])

  useEffect(() => {
    if (provider) {
      setName(provider.name)
      setApiUrl(provider.api_url)
      setApiKey("")
      setSelectedTypes([...provider.agent_types])
      setError(null)
    }
  }, [provider])

  const handleOpenChange = useCallback(
    (nextOpen: boolean) => {
      if (!nextOpen) setError(null)
      onOpenChange(nextOpen)
    },
    [onOpenChange]
  )

  const toggleAgentType = useCallback((at: AgentType) => {
    setSelectedTypes((prev) =>
      prev.includes(at) ? prev.filter((t) => t !== at) : [...prev, at]
    )
  }, [])

  const handleSubmit = useCallback(async () => {
    if (!provider) return
    if (!name.trim()) {
      setError(t("nameRequired"))
      return
    }
    if (!apiUrl.trim()) {
      setError(t("apiUrlRequired"))
      return
    }
    if (selectedTypes.length === 0) {
      setError(t("agentTypesRequired"))
      return
    }

    setLoading(true)
    setError(null)
    try {
      await updateModelProvider({
        id: provider.id,
        name: name.trim() !== provider.name ? name.trim() : undefined,
        apiUrl: apiUrl.trim() !== provider.api_url ? apiUrl.trim() : undefined,
        apiKey: apiKey.trim() || undefined,
        agentTypes:
          JSON.stringify(selectedTypes) !== JSON.stringify(provider.agent_types)
            ? selectedTypes
            : undefined,
      })
      toast.success(t("editSuccess"))
      handleOpenChange(false)
      onProviderUpdated()
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setError(msg)
    } finally {
      setLoading(false)
    }
  }, [
    provider,
    name,
    apiUrl,
    apiKey,
    selectedTypes,
    handleOpenChange,
    onProviderUpdated,
    t,
  ])

  return (
    <Dialog open={!!provider} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("editProvider")}</DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1.5">
            <label htmlFor="edit-mp-name" className="text-xs font-medium">
              {t("providerName")}
            </label>
            <Input
              id="edit-mp-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("providerNamePlaceholder")}
            />
          </div>

          <div className="space-y-1.5">
            <label htmlFor="edit-mp-url" className="text-xs font-medium">
              {t("apiUrl")}
            </label>
            <Input
              id="edit-mp-url"
              value={apiUrl}
              onChange={(e) => setApiUrl(e.target.value)}
              placeholder={t("apiUrlPlaceholder")}
            />
          </div>

          <div className="space-y-1.5">
            <label htmlFor="edit-mp-key" className="text-xs font-medium">
              {t("apiKey")}
            </label>
            <Input
              id="edit-mp-key"
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder={t("apiKeyKeepCurrent")}
            />
          </div>

          <div className="space-y-1.5">
            <label className="text-xs font-medium">{t("agentTypes")}</label>
            <div className="flex flex-wrap gap-1.5">
              {ALL_AGENT_TYPES.map((at) => (
                <Button
                  key={at}
                  type="button"
                  size="sm"
                  variant={selectedTypes.includes(at) ? "default" : "outline"}
                  className="h-7 text-xs"
                  aria-pressed={selectedTypes.includes(at)}
                  onClick={() => toggleAgentType(at)}
                >
                  {AGENT_LABELS[at]}
                </Button>
              ))}
            </div>
          </div>

          {error && (
            <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
              {error}
            </div>
          )}
        </div>

        <DialogFooter>
          <Button
            variant="outline"
            onClick={() => handleOpenChange(false)}
            disabled={loading}
          >
            {t("cancel")}
          </Button>
          <Button onClick={handleSubmit} disabled={loading}>
            {loading && <Loader2 className="h-3.5 w-3.5 animate-spin mr-1" />}
            {t("save")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
