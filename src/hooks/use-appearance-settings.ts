"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTheme } from "next-themes"
import {
  type AppearanceSettings,
  type ThemeColor,
  APPEARANCE_STORAGE_KEY,
  APPEARANCE_UPDATED_EVENT,
  CODE_FONT_SIZE_MAX,
  CODE_FONT_SIZE_MIN,
  DEFAULT_APPEARANCE,
  UI_FONT_SIZE_MAX,
  UI_FONT_SIZE_MIN,
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
  const [appearance, setAppearance] =
    useState<AppearanceSettings>(DEFAULT_APPEARANCE)
  const { resolvedTheme } = useTheme()
  const appearanceRef = useRef(appearance)

  useEffect(() => {
    appearanceRef.current = appearance
  }, [appearance])

  // Re-apply theme color when dark/light mode changes
  useEffect(() => {
    applyAppearanceSettings(appearanceRef.current)
  }, [resolvedTheme])

  useEffect(() => {
    const syncFromStorage = () => {
      const settings = readAppearanceSettings()
      setAppearance(settings)
      applyAppearanceSettings(settings)
    }

    syncFromStorage()

    const onStorage = (event: StorageEvent) => {
      if (event.key && event.key !== APPEARANCE_STORAGE_KEY) return
      syncFromStorage()
    }

    window.addEventListener("storage", onStorage)
    window.addEventListener(APPEARANCE_UPDATED_EVENT, syncFromStorage)
    return () => {
      window.removeEventListener("storage", onStorage)
      window.removeEventListener(APPEARANCE_UPDATED_EVENT, syncFromStorage)
    }
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
    (size: number) =>
      update({
        uiFontSize: Math.round(
          Math.min(Math.max(size, UI_FONT_SIZE_MIN), UI_FONT_SIZE_MAX)
        ),
      }),
    [update]
  )

  const updateCodeFontSize = useCallback(
    (size: number) =>
      update({
        codeFontSize: Math.round(
          Math.min(Math.max(size, CODE_FONT_SIZE_MIN), CODE_FONT_SIZE_MAX)
        ),
      }),
    [update]
  )

  const resetAppearance = useCallback(() => {
    setAppearance({ ...DEFAULT_APPEARANCE })
    writeAppearanceSettings(DEFAULT_APPEARANCE)
    applyAppearanceSettings(DEFAULT_APPEARANCE)
  }, [])

  return {
    appearance,
    updateThemeColor,
    updateUiFontSize,
    updateCodeFontSize,
    resetAppearance,
  }
}
