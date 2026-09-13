"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { useImeGuard } from "@/hooks/use-ime-guard"

interface CustomModelIdDialogProps {
  open: boolean
  onClose: () => void
  /** Called with the trimmed id. The caller routes it through the exact same
   *  path as picking an advertised option. */
  onSubmit: (modelId: string) => void
}

// Free-text entry behind the model picker's "Use custom model id..." row. A
// brand-new model is often live on the wire days before the agent's curated
// list catches up, so the only thing between the user and the model is a place
// to type the id. Deliberately no validation beyond trim/non-empty: the id is
// sent verbatim and the agent's own verdict — adopt it, settle somewhere else
// (`config_option_rejected`), or refuse the set outright — comes back through
// the existing config-option flow, for every agent alike.
export function CustomModelIdDialog({
  open,
  onClose,
  onSubmit,
}: CustomModelIdDialogProps) {
  const t = useTranslations("Folder.chat.messageInput")
  const ime = useImeGuard()
  const [value, setValue] = useState("")
  const trimmed = value.trim()

  function handleClose() {
    setValue("")
    onClose()
  }

  function handleSubmit() {
    if (!trimmed) return
    setValue("")
    onSubmit(trimmed)
  }

  return (
    <Dialog open={open} onOpenChange={(v) => !v && handleClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle>{t("customModelTitle")}</DialogTitle>
          <DialogDescription>{t("customModelDescription")}</DialogDescription>
        </DialogHeader>

        <div className="space-y-2">
          <Label htmlFor="custom-model-id">{t("customModelInputLabel")}</Label>
          <Input
            id="custom-model-id"
            placeholder={t("customModelPlaceholder")}
            value={value}
            onChange={(e) => setValue(e.target.value)}
            {...ime.props}
            onKeyDown={(e) => {
              if (ime.isComposing(e)) return
              if (e.key === "Enter") handleSubmit()
            }}
            spellCheck={false}
            autoComplete="off"
            autoFocus
          />
        </div>

        <DialogFooter>
          <Button variant="outline" size="sm" onClick={handleClose}>
            {t("cancel")}
          </Button>
          <Button size="sm" onClick={handleSubmit} disabled={!trimmed}>
            {t("customModelApply")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
