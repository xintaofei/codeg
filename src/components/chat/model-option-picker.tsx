"use client"

import { useMemo, useState } from "react"
import { ChevronDown, Settings2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import { ModelOptionList } from "@/components/chat/model-option-list"
import { KiloChatModelManager } from "@/components/chat/kilo-chat-model-manager"
import { useScrollbarSafeDismiss } from "@/hooks/use-scrollbar-safe-dismiss"
import type { ModelOptionGroup } from "@/lib/model-config-groups"
import {
  isKiloAgentType,
  type AgentType,
  type SessionConfigOptionInfo,
} from "@/lib/types"

interface ModelOptionPickerProps {
  option: SessionConfigOptionInfo
  /** The grouped list to show (derived `provider/` groups, or a single
   *  headerless group for a long flat list). */
  groups: ModelOptionGroup[]
  onSelect: (configId: string, valueId: string) => void
  agentType?: AgentType | null
}

// Wide-form model picker for LONG model lists: a trigger button opening a
// Popover that hosts the searchable + virtualized {@link ModelOptionList}.
// Replaces the Radix `DropdownMenu` (whose roving focus over hundreds of items
// is the scroll jank) only for the model option, only when it's large — short
// lists keep `InlineSessionConfigSelector`. Mirrors the BranchPicker layout
// (Popover `overflow-hidden p-0`, the list is the sole nested scroller).
// Grabbing the list's scrollbar would otherwise dismiss the popover on WebKit —
// the grab blurs focus and WebKit bounces it to an outside element, which Radix
// reads as a focus-outside; `useScrollbarSafeDismiss` keeps it open (see there).
export function ModelOptionPicker({
  option,
  groups,
  onSelect,
  agentType,
}: ModelOptionPickerProps) {
  const t = useTranslations("Folder.chat.messageInput")
  const [open, setOpen] = useState(false)
  const [manageOpen, setManageOpen] = useState(false)
  const { contentRef, onPointerDownOutside, onFocusOutside } =
    useScrollbarSafeDismiss()
  const kind = option.kind.type === "select" ? option.kind : null
  const currentValue = kind?.current_value ?? ""
  const currentLabel = useMemo(() => {
    for (const group of groups) {
      for (const opt of group.options) {
        if (opt.value === currentValue) return opt.name
      }
    }
    return currentValue
  }, [groups, currentValue])

  if (!kind) return null

  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button
          variant="ghost"
          size="xs"
          title={option.name}
          aria-label={
            currentLabel ? `${option.name}: ${currentLabel}` : option.name
          }
          className="min-w-0 gap-0.5 px-1 text-muted-foreground"
        >
          <span className="max-w-[10rem] truncate">{currentLabel}</span>
          <ChevronDown className="size-3 shrink-0 text-muted-foreground" />
        </Button>
      </PopoverTrigger>
      <PopoverContent
        ref={contentRef}
        side="top"
        align="start"
        onPointerDownOutside={onPointerDownOutside}
        onFocusOutside={onFocusOutside}
        className="w-[22rem] max-w-[calc(100vw-1rem)] overflow-hidden p-0"
      >
        <ModelOptionList
          groups={groups}
          currentValue={currentValue}
          onSelect={(value) => {
            onSelect(option.id, value)
            setOpen(false)
          }}
          searchPlaceholder={t("searchModel")}
          searchAriaLabel={t("searchModelAria")}
          listAriaLabel={t("modelListLabel")}
          emptyLabel={t("noModels")}
          autoFocus
          collapsibleGroups={Boolean(agentType && isKiloAgentType(agentType))}
        />
        {agentType && isKiloAgentType(agentType) && (
          <div className="border-t p-2">
            <Button
              type="button"
              size="sm"
              variant="ghost"
              className="w-full justify-start"
              onClick={() => {
                setOpen(false)
                setManageOpen(true)
              }}
            >
              <Settings2 className="size-3.5" />
              {t("manageKiloModels")}
            </Button>
          </div>
        )}
      </PopoverContent>
      <KiloChatModelManager open={manageOpen} onOpenChange={setManageOpen} />
    </Popover>
  )
}
