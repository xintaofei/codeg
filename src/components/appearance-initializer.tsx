"use client"

import { useEffect } from "react"
import { useTheme } from "next-themes"
import {
  APPEARANCE_STORAGE_KEY,
  APPEARANCE_UPDATED_EVENT,
  applyAppearanceSettings,
  readAppearanceSettings,
} from "@/lib/appearance-settings"

/**
 * Global appearance settings initializer. Must be mounted once in the root
 * layout so that font-size and theme-color changes propagate to every page,
 * not just the settings panel.
 */
export function AppearanceInitializer() {
  const { resolvedTheme } = useTheme()

  useEffect(() => {
    const apply = () => applyAppearanceSettings(readAppearanceSettings())
    apply()

    const onStorage = (e: StorageEvent) => {
      if (e.key && e.key !== APPEARANCE_STORAGE_KEY) return
      apply()
    }
    window.addEventListener(APPEARANCE_UPDATED_EVENT, apply)
    window.addEventListener("storage", onStorage)
    return () => {
      window.removeEventListener(APPEARANCE_UPDATED_EVENT, apply)
      window.removeEventListener("storage", onStorage)
    }
  }, [resolvedTheme])

  return null
}
