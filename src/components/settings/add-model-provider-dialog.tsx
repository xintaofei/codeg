"use client"

import { useCallback, useState } from "react"
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
import { createModelProvider } from "@/lib/api"
import { ALL_AGENT_TYPES, AGENT_LABELS, type AgentType } from "@/lib/types"

interface AddModelProviderDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onProviderAdded: () => void
}

export function AddModelProviderDialog({
  open,
  onOpenChange,
  onProviderAdded,
}: AddModelProviderDialogProps) {
  const t = useTranslations("ModelProviderSettings")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const [name, setName] = useState("")
  const [apiUrl, setApiUrl] = useState("")
  const [apiKey, setApiKey] = useState("")
  const [selectedTypes, setSelectedTypes] = useState<AgentType[]>([])

  const resetForm = useCallback(() => {
    setName("")
    setApiUrl("")
    setApiKey("")
    setSelectedTypes([])
    setError(null)
  }, [])

  const handleOpenChange = useCallback(
    (nextOpen: boolean) => {
      if (!nextOpen) resetForm()
      onOpenChange(nextOpen)
    },
    [onOpenChange, resetForm]
  )

  const toggleAgentType = useCallback((at: AgentType) => {
    setSelectedTypes((prev) =>
      prev.includes(at) ? prev.filter((t) => t !== at) : [...prev, at]
    )
  }, [])

  const handleSubmit = useCallback(async () => {
    if (!name.trim()) {
      setError(t("nameRequired"))
      return
    }
    if (!apiUrl.trim()) {
      setError(t("apiUrlRequired"))
      return
    }
    if (!apiKey.trim()) {
      setError(t("apiKeyRequired"))
      return
    }
    if (selectedTypes.length === 0) {
      setError(t("agentTypesRequired"))
      return
    }

    setLoading(true)
    setError(null)
    try {
      await createModelProvider({
        name: name.trim(),
        apiUrl: apiUrl.trim(),
        apiKey: apiKey.trim(),
        agentTypes: selectedTypes,
      })
      toast.success(t("createSuccess"))
      handleOpenChange(false)
      onProviderAdded()
    } catch (err) {
      const msg = err instanceof Error ? err.message : String(err)
      setError(msg)
    } finally {
      setLoading(false)
    }
  }, [
    name,
    apiUrl,
    apiKey,
    selectedTypes,
    handleOpenChange,
    onProviderAdded,
    t,
  ])

  return (
    <Dialog open={open} onOpenChange={handleOpenChange}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("addProvider")}</DialogTitle>
        </DialogHeader>

        <div className="space-y-4">
          <div className="space-y-1.5">
            <label htmlFor="add-mp-name" className="text-xs font-medium">
              {t("providerName")}
            </label>
            <Input
              id="add-mp-name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder={t("providerNamePlaceholder")}
            />
          </div>

          <div className="space-y-1.5">
            <label htmlFor="add-mp-url" className="text-xs font-medium">
              {t("apiUrl")}
            </label>
            <Input
              id="add-mp-url"
              value={apiUrl}
              onChange={(e) => setApiUrl(e.target.value)}
              placeholder={t("apiUrlPlaceholder")}
            />
          </div>

          <div className="space-y-1.5">
            <label htmlFor="add-mp-key" className="text-xs font-medium">
              {t("apiKey")}
            </label>
            <Input
              id="add-mp-key"
              type="password"
              value={apiKey}
              onChange={(e) => setApiKey(e.target.value)}
              placeholder={t("apiKeyPlaceholder")}
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
            {t("create")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
