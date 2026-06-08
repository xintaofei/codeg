"use client"

/**
 * Dialog for composing a live-feedback note, opened from the composer "+" menu.
 * Add-only (notes are read-only once sent). Enter sends, Shift+Enter inserts a
 * newline — matching the main composer.
 *
 * The draft lives in `FeedbackDialogForm`, which is mounted only inside
 * `DialogContent` (Radix unmounts it on close), so every open starts with an
 * empty field — no reset effect needed.
 */

import { useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2, Send } from "lucide-react"

import { Button } from "@/components/ui/button"
import { Textarea } from "@/components/ui/textarea"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

interface FeedbackDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  onSubmit: (text: string) => void
  submitting?: boolean
  agentName?: string
}

interface FeedbackDialogFormProps {
  onSubmit: (text: string) => void
  onCancel: () => void
  submitting: boolean
  agentName?: string
}

function FeedbackDialogForm({
  onSubmit,
  onCancel,
  submitting,
  agentName,
}: FeedbackDialogFormProps) {
  const t = useTranslations("LiveFeedback")
  const [text, setText] = useState("")
  const composingRef = useRef(false)

  const trimmed = text.trim()
  const canSend = trimmed.length > 0 && !submitting

  const handleSubmit = () => {
    if (canSend) onSubmit(trimmed)
  }

  return (
    <>
      <DialogHeader>
        <DialogTitle>{t("dialogTitle")}</DialogTitle>
        <DialogDescription>{t("dialogDescription")}</DialogDescription>
      </DialogHeader>
      <Textarea
        autoFocus
        value={text}
        onChange={(e) => setText(e.target.value)}
        onCompositionStart={() => (composingRef.current = true)}
        onCompositionEnd={() => (composingRef.current = false)}
        onKeyDown={(e) => {
          if (
            e.nativeEvent.isComposing ||
            composingRef.current ||
            e.key === "Process" ||
            e.keyCode === 229
          )
            return
          if (e.key === "Enter" && !e.shiftKey) {
            e.preventDefault()
            handleSubmit()
          }
        }}
        placeholder={t("placeholder", {
          agent: agentName ?? t("agentFallback"),
        })}
        aria-label={t("ariaLabel")}
        className="min-h-28"
      />
      <DialogFooter>
        <Button variant="ghost" onClick={onCancel}>
          {t("cancel")}
        </Button>
        <Button onClick={handleSubmit} disabled={!canSend}>
          {submitting ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <Send className="size-4" />
          )}
          {t("send")}
        </Button>
      </DialogFooter>
    </>
  )
}

export function FeedbackDialog({
  open,
  onOpenChange,
  onSubmit,
  submitting = false,
  agentName,
}: FeedbackDialogProps) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="sm:max-w-lg">
        <FeedbackDialogForm
          onSubmit={onSubmit}
          onCancel={() => onOpenChange(false)}
          submitting={submitting}
          agentName={agentName}
        />
      </DialogContent>
    </Dialog>
  )
}
