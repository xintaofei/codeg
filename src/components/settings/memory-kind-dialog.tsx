"use client"

import { useEffect, useReducer } from "react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { Textarea } from "@/components/ui/textarea"
import type { MemoryKind, MemoryKindDraft, MemoryMode } from "@/lib/types"

interface MemoryKindDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  editingKind?: MemoryKind | null
  onSave: (draft: MemoryKindDraft) => Promise<void> | void
  saving?: boolean
}

export function MemoryKindDialog({
  open,
  onOpenChange,
  editingKind,
  onSave,
  saving = false,
}: MemoryKindDialogProps) {
  const t = useTranslations("Memory")

  interface FormState {
    name: string
    instruction: string
    mode: MemoryMode
  }

  const formReducer = (
    state: FormState,
    action:
      | { type: "setName"; payload: string }
      | { type: "setInstruction"; payload: string }
      | { type: "setMode"; payload: MemoryMode }
      | { type: "reset"; payload: FormState }
  ): FormState => {
    switch (action.type) {
      case "setName":
        return { ...state, name: action.payload }
      case "setInstruction":
        return { ...state, instruction: action.payload }
      case "setMode":
        return { ...state, mode: action.payload }
      case "reset":
        return action.payload
      default:
        return state
    }
  }

  const [formState, formDispatch] = useReducer(formReducer, {
    name: "",
    instruction: "",
    mode: "auto" as MemoryMode,
  })

  const setName = (value: string) =>
    formDispatch({ type: "setName", payload: value })
  const setInstruction = (value: string) =>
    formDispatch({ type: "setInstruction", payload: value })
  const setMode = (value: MemoryMode) =>
    formDispatch({ type: "setMode", payload: value })

  const { name, instruction, mode } = formState

  useEffect(() => {
    if (open) {
      if (editingKind) {
        formDispatch({
          type: "reset",
          payload: {
            name: editingKind.name,
            instruction: editingKind.instruction,
            mode: editingKind.mode,
          },
        })
      } else {
        formDispatch({
          type: "reset",
          payload: {
            name: "",
            instruction: "",
            mode: "auto",
          },
        })
      }
    }
  }, [open, editingKind])

  const handleSubmit = async (e: React.FormEvent) => {
    e.preventDefault()
    const trimmedName = name.trim()
    const trimmedInstruction = instruction.trim()
    if (!trimmedName || !trimmedInstruction || saving) return

    await onSave({
      name: trimmedName,
      instruction: trimmedInstruction,
      mode,
    })
  }

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-[480px]">
        <form onSubmit={handleSubmit} className="space-y-4">
          <DialogHeader>
            <DialogTitle>
              {editingKind ? t("editKind") : t("addKind")}
            </DialogTitle>
            <DialogDescription className="text-xs">
              {t("kindsHint")}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4 py-2">
            <div className="space-y-1.5">
              <Label htmlFor="memory-kind-name">{t("kindName")}</Label>
              <Input
                id="memory-kind-name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                placeholder={t("kindName")}
                disabled={saving}
                autoFocus
                required
              />
            </div>

            <div className="space-y-1.5">
              <Label htmlFor="memory-kind-instruction">
                {t("kindInstruction")}
              </Label>
              <Textarea
                id="memory-kind-instruction"
                value={instruction}
                onChange={(e) => setInstruction(e.target.value)}
                placeholder={t("kindInstructionPlaceholder")}
                rows={4}
                disabled={saving}
                required
              />
            </div>

            <div className="space-y-2">
              <Label>{t("mode")}</Label>
              <RadioGroup
                value={mode}
                onValueChange={(val) => setMode(val as MemoryMode)}
                disabled={saving}
                className="gap-2"
              >
                <label
                  htmlFor="mode-auto"
                  className="flex items-start gap-3 rounded-lg border border-border/70 p-2.5 transition-colors hover:bg-muted/50 cursor-pointer has-data-[state=checked]:border-primary has-data-[state=checked]:bg-primary/5"
                >
                  <RadioGroupItem
                    value="auto"
                    id="mode-auto"
                    className="mt-0.5"
                  />
                  <div className="min-w-0 flex-1 space-y-0.5">
                    <div className="text-xs font-medium">{t("modeAuto")}</div>
                    <div className="text-2xs text-muted-foreground">
                      {t("modeHintAuto")}
                    </div>
                  </div>
                </label>

                <label
                  htmlFor="mode-on-request"
                  className="flex items-start gap-3 rounded-lg border border-border/70 p-2.5 transition-colors hover:bg-muted/50 cursor-pointer has-data-[state=checked]:border-primary has-data-[state=checked]:bg-primary/5"
                >
                  <RadioGroupItem
                    value="on_request"
                    id="mode-on-request"
                    className="mt-0.5"
                  />
                  <div className="min-w-0 flex-1 space-y-0.5">
                    <div className="text-xs font-medium">
                      {t("modeOnRequest")}
                    </div>
                    <div className="text-2xs text-muted-foreground">
                      {t("modeHintOnRequest")}
                    </div>
                  </div>
                </label>

                <label
                  htmlFor="mode-off"
                  className="flex items-start gap-3 rounded-lg border border-border/70 p-2.5 transition-colors hover:bg-muted/50 cursor-pointer has-data-[state=checked]:border-primary has-data-[state=checked]:bg-primary/5"
                >
                  <RadioGroupItem
                    value="off"
                    id="mode-off"
                    className="mt-0.5"
                  />
                  <div className="min-w-0 flex-1 space-y-0.5">
                    <div className="text-xs font-medium">{t("modeOff")}</div>
                  </div>
                </label>
              </RadioGroup>
            </div>
          </div>

          <DialogFooter>
            <Button
              type="button"
              variant="outline"
              onClick={() => onOpenChange(false)}
              disabled={saving}
            >
              {t("cancel")}
            </Button>
            <Button
              type="submit"
              disabled={!name.trim() || !instruction.trim() || saving}
            >
              {saving ? "..." : t("save")}
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
