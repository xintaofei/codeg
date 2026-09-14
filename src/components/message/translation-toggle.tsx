"use client"

import { useTranslations } from "next-intl"
import { Languages } from "lucide-react"

import { Button } from "@/components/ui/button"
import { useTranslationSettingsSnapshot } from "@/hooks/use-translated-text"
import { cn } from "@/lib/utils"

interface TranslationToggleProps {
  /** True when the message currently shows the translation. */
  isTranslated: boolean
  onShowOriginal: () => void
  onShowTranslation: () => void
  className?: string
  /**
   * Why this block's translation is incomplete, when chunks failed and the
   * endpoint never delivered. Rendered as an amber indicator on the toggle
   * with the reason on hover — the first place a "why is this still English"
   * reader can look, instead of the browser console.
   */
  warning?: string | null
}

/**
 * Switch between the original and the translation. The action shown is
 * whatever the message is *not* currently displaying. Hover-revealed by
 * default; the settings page's "always visible" switch removes the hover
 * gate (the snapshot read is one subscription shared by every toggle).
 */
export function TranslationToggle({
  isTranslated,
  onShowOriginal,
  onShowTranslation,
  className,
  warning,
}: TranslationToggleProps) {
  const t = useTranslations("Translation")
  const { toggleAlwaysVisible } = useTranslationSettingsSnapshot()

  return (
    <Button
      variant="ghost"
      size="icon-sm"
      className={cn(
        // Keyboard parity with hover (see generated-images-block.tsx): a
        // Tab-focused toggle must become visible, and so must the toggle when
        // anything inside its message part holds focus.
        toggleAlwaysVisible
          ? "opacity-100"
          : "opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 group-focus-within:opacity-100",
        className
      )}
      type="button"
      aria-label={isTranslated ? t("showOriginal") : t("showTranslation")}
      title={
        warning
          ? `${isTranslated ? t("showOriginal") : t("showTranslation")} — ${warning}`
          : isTranslated
            ? t("showOriginal")
            : t("showTranslation")
      }
      onClick={isTranslated ? onShowOriginal : onShowTranslation}
    >
      <Languages
        className={cn(
          "size-4",
          warning && "text-amber-600 dark:text-amber-400"
        )}
      />
    </Button>
  )
}
