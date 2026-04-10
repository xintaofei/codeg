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

  // Apply on mount + whenever the custom event fires (same tab)
  useEffect(() => {
    const apply = () => applyAppearanceSettings(readAppearanceSettings())
    apply()

    window.addEventListener(APPEARANCE_UPDATED_EVENT, apply)
    return () => window.removeEventListener(APPEARANCE_UPDATED_EVENT, apply)
  }, [])

  // Re-apply when dark/light mode toggles so theme-color overrides match
  useEffect(() => {
    applyAppearanceSettings(readAppearanceSettings())
  }, [resolvedTheme])

  // Cross-tab sync via StorageEvent
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key && e.key !== APPEARANCE_STORAGE_KEY) return
      applyAppearanceSettings(readAppearanceSettings())
    }
    window.addEventListener("storage", onStorage)
    return () => window.removeEventListener("storage", onStorage)
  }, [])

  return null
}
