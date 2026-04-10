"use client"

import { useCallback, useEffect, useState } from "react"
import {
  type AppearanceSettings,
  type ThemeColor,
  APPEARANCE_STORAGE_KEY,
  DEFAULT_APPEARANCE,
  applyAppearanceSettings,
  readAppearanceSettings,
  writeAppearanceSettings,
} from "@/lib/appearance-settings"

interface UseAppearanceSettingsResult {
  appearance: AppearanceSettings
  updateThemeColor: (color: ThemeColor) => void
  updateUiFontSize: (size: number) => void
  updateCodeFontSize: (size: number) => void
  resetAppearance: () => void
}

export function useAppearanceSettings(): UseAppearanceSettingsResult {
  const [appearance, setAppearance] = useState<AppearanceSettings>(
    readAppearanceSettings
  )

  useEffect(() => {
    const onStorage = (event: StorageEvent) => {
      if (event.key && event.key !== APPEARANCE_STORAGE_KEY) return
      setAppearance(readAppearanceSettings())
    }
    window.addEventListener("storage", onStorage)
    return () => window.removeEventListener("storage", onStorage)
  }, [])

  const update = useCallback((patch: Partial<AppearanceSettings>) => {
    setAppearance((prev) => {
      const next = { ...prev, ...patch }
      writeAppearanceSettings(next)
      applyAppearanceSettings(next)
      return next
    })
  }, [])

  const updateThemeColor = useCallback(
    (color: ThemeColor) => update({ themeColor: color }),
    [update]
  )

  const updateUiFontSize = useCallback(
    (size: number) => update({ uiFontSize: size }),
    [update]
  )

  const updateCodeFontSize = useCallback(
    (size: number) => update({ codeFontSize: size }),
    [update]
  )

  const resetAppearance = useCallback(() => {
    writeAppearanceSettings(DEFAULT_APPEARANCE)
    applyAppearanceSettings(DEFAULT_APPEARANCE)
    setAppearance({ ...DEFAULT_APPEARANCE })
  }, [])

  return {
    appearance,
    updateThemeColor,
    updateCodeFontSize,
    updateUiFontSize,
    resetAppearance,
  }
}
