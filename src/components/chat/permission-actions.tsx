"use client"

import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import type { PermissionOptionInfo } from "@/lib/types"

type PermissionActionVariant = "default" | "outline"
type ActionLabelKey = (typeof KIND_LABEL_KEYS)[keyof typeof KIND_LABEL_KEYS]

interface PermissionActionsProps {
  options: PermissionOptionInfo[]
  onRespond: (optionId: string) => void
}

const KIND_LABEL_KEYS = {
  allow_once: "allowOnce",
  allow_always: "allowAlways",
  reject_once: "rejectOnce",
  reject_always: "rejectAlways",
} as const

const KIND_VARIANTS: Record<string, PermissionActionVariant> = {
  allow_once: "default",
  allow_always: "default",
  reject_once: "outline",
  reject_always: "outline",
}

function extractDetail(name: string): string | undefined {
  const match = name.match(/`([^`]+)`/)
  return match?.[1]
}

export function PermissionActions({
  options,
  onRespond,
}: PermissionActionsProps) {
  const t = useTranslations("Folder.chat.permissionDialog.actions")

  return (
    <div className="mt-3 flex flex-wrap gap-2">
      {options.map((opt) => {
        const variant: PermissionActionVariant =
          KIND_VARIANTS[opt.kind] ??
          (opt.kind.startsWith("reject") ? "outline" : "default")
        const labelKey =
          KIND_LABEL_KEYS[opt.kind as keyof typeof KIND_LABEL_KEYS]
        const label = labelKey ? t(labelKey as ActionLabelKey) : opt.name
        // Only split label/detail for known kinds; unrecognized kinds
        // render opt.name as-is since we have no translation key for them.
        const detail = labelKey ? extractDetail(opt.name) : undefined

        if (detail) {
          return (
            <Button
              key={opt.option_id}
              variant={variant}
              className="h-auto min-h-9 max-w-full basis-full justify-start overflow-hidden text-left"
              title={opt.name}
              onClick={() => onRespond(opt.option_id)}
            >
              <span className="shrink-0">{label} ·</span>
              <code className="truncate text-[0.85em] opacity-70">
                {detail}
              </code>
            </Button>
          )
        }

        return (
          <Button
            key={opt.option_id}
            variant={variant}
            className="h-auto min-h-9 whitespace-normal break-words"
            title={opt.name}
            onClick={() => onRespond(opt.option_id)}
          >
            {label}
          </Button>
        )
      })}
    </div>
  )
}
