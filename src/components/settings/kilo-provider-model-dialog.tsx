"use client"

import { useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"

interface KiloProviderModelDialogProps {
  open: boolean
  providerId: string
  modelIds: string[]
  onOpenChange: (open: boolean) => void
  onAdd: (modelIds: string[]) => void
}
export function KiloProviderModelDialog({
  open,
  providerId,
  modelIds,
  onOpenChange,
  onAdd,
}: KiloProviderModelDialogProps) {
  const t = useTranslations("AcpAgentSettings")
  const [search, setSearch] = useState("")
  const [selection, setSelection] = useState<string[]>([])

  const filteredModelIds = useMemo(() => {
    const query = search.trim().toLowerCase()
    if (!query) return modelIds
    return modelIds.filter((modelId) => modelId.toLowerCase().includes(query))
  }, [modelIds, search])

  const handleAdd = () => {
    if (selection.length === 0) return
    onAdd(selection)
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-lg">
        <DialogHeader>
          <DialogTitle>{t("openCode.chooseModelsTitle")}</DialogTitle>
          <DialogDescription>
            {t("openCode.chooseModelsDescription", { providerId })}
          </DialogDescription>
        </DialogHeader>

        <Input
          value={search}
          onChange={(event) => setSearch(event.target.value)}
          placeholder={t("openCode.searchModels")}
          autoFocus
        />

        <div className="flex items-center justify-between gap-2 text-xs text-muted-foreground">
          <span>
            {t("openCode.selectedModels", {
              selected: selection.length,
              total: modelIds.length,
            })}
          </span>
          <div className="flex gap-2">
            <button
              type="button"
              className="hover:text-foreground"
              onClick={() => setSelection(modelIds)}
            >
              {t("openCode.selectAllModels")}
            </button>
            <button
              type="button"
              className="hover:text-foreground"
              onClick={() => setSelection([])}
            >
              {t("openCode.clearSelectedModels")}
            </button>
          </div>
        </div>

        <div className="max-h-72 space-y-1 overflow-y-auto rounded-md border p-1">
          {filteredModelIds.length === 0 ? (
            <p className="px-2 py-4 text-center text-sm text-muted-foreground">
              {t("openCode.noMatchingModels")}
            </p>
          ) : (
            filteredModelIds.map((modelId) => {
              const selected = selection.includes(modelId)
              return (
                <label
                  key={modelId}
                  className="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-sm hover:bg-accent"
                >
                  <Checkbox
                    checked={selected}
                    onCheckedChange={(checked) => {
                      setSelection((current) =>
                        checked
                          ? [...current, modelId]
                          : current.filter((id) => id !== modelId)
                      )
                    }}
                  />
                  <span className="min-w-0 truncate font-mono text-xs">
                    {modelId}
                  </span>
                </label>
              )
            })
          )}
        </div>

        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
          >
            {t("actions.cancel")}
          </Button>
          <Button
            type="button"
            disabled={selection.length === 0}
            onClick={handleAdd}
          >
            {t("openCode.addSelectedModels")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
