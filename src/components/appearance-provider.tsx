"use client"

import { createContext, useCallback, useEffect, useRef, useState } from "react"
import {
  getSystemFontSettings,
  listSystemFontFamilies,
  updateSystemFontSettings,
} from "@/lib/api"
import {
  THEME_COLORS,
  DEFAULT_THEME_COLOR,
  type ThemeColor,
  ZOOM_LEVELS,
  DEFAULT_ZOOM_LEVEL,
  type ZoomLevel,
  BUILT_IN_CODE_FONT_FAMILIES,
  DEFAULT_CODE_FONT_FAMILY,
  DEFAULT_UI_FONT_FAMILY,
  buildCodeFontFamilyStack,
  buildUiFontFamilyStack,
  isBuiltInFontFamilyOption,
  normalizeFontFamilyPreference,
  type FontFamilyPreference,
} from "@/lib/theme-presets"
import {
  STORAGE_KEY_CODE_FONT_FAMILY,
  STORAGE_KEY_THEME_COLOR,
  STORAGE_KEY_UI_FONT_FAMILY,
  STORAGE_KEY_ZOOM_LEVEL,
} from "@/lib/appearance-script"
import type { SystemFontFamily, SystemFontSettings } from "@/lib/types"

function syncTrafficLightPosition(zoom: number) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window))
    return
  import("@/lib/tauri").then((t) =>
    t.updateTrafficLightPosition(zoom).catch(() => {})
  )
}

function syncAppearanceMode(mode: string) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window))
    return
  import("@/lib/tauri").then((t) =>
    t.updateAppearanceMode(mode).catch(() => {})
  )
}

function isTauriDesktop() {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window
}

function applyUiFontFamily(fontFamily: FontFamilyPreference) {
  const normalized = normalizeFontFamilyPreference(fontFamily)
  const root = document.documentElement
  root.style.setProperty(
    "--codeg-ui-font-family",
    buildUiFontFamilyStack(normalized)
  )
  if (normalized) {
    root.dataset.uiFontFamily = normalized
  } else {
    root.removeAttribute("data-ui-font-family")
  }
}

function applyCodeFontFamily(fontFamily: FontFamilyPreference) {
  const normalized = normalizeFontFamilyPreference(fontFamily)
  const root = document.documentElement
  root.style.setProperty(
    "--codeg-code-font-family",
    buildCodeFontFamilyStack(normalized)
  )
  if (normalized) {
    root.dataset.codeFontFamily = normalized
  } else {
    root.removeAttribute("data-code-font-family")
  }
}

function readFontFamilyFromDataset(
  key: "uiFontFamily" | "codeFontFamily"
): FontFamilyPreference {
  if (typeof document === "undefined") return null
  return normalizeFontFamilyPreference(document.documentElement.dataset[key])
}

function writeFontFamilyToStorage(
  key: string,
  fontFamily: FontFamilyPreference
) {
  if (typeof window === "undefined") return
  const normalized = normalizeFontFamilyPreference(fontFamily)
  try {
    if (normalized) {
      localStorage.setItem(key, normalized)
    } else {
      localStorage.removeItem(key)
    }
  } catch {
    // 隐私模式 / 禁用 storage 时静默忽略，本次会话内仍然生效
  }
}

function readFontFamilyStorageState(key: string): {
  hasStoredValue: boolean
  fontFamily: FontFamilyPreference
} {
  try {
    const raw = localStorage.getItem(key)
    return {
      hasStoredValue: raw !== null,
      fontFamily: normalizeFontFamilyPreference(raw),
    }
  } catch {
    return { hasStoredValue: false, fontFamily: null }
  }
}

function isKnownFontFamily(
  fontFamily: FontFamilyPreference,
  families: SystemFontFamily[]
): boolean {
  if (!fontFamily) return true
  const key = fontFamily.toLowerCase()
  return (
    isBuiltInFontFamilyOption(fontFamily) ||
    families.some((option) => option.family.toLowerCase() === key)
  )
}

function isKnownCodeFontFamily(
  fontFamily: FontFamilyPreference,
  families: SystemFontFamily[]
): boolean {
  if (!fontFamily) return true
  const key = fontFamily.toLowerCase()
  return (
    BUILT_IN_CODE_FONT_FAMILIES.some(
      (family) => family.toLowerCase() === key
    ) ||
    families.some(
      (option) => option.monospace && option.family.toLowerCase() === key
    )
  )
}

function normalizeSystemFontSettings(settings: SystemFontSettings): {
  uiFontFamily: FontFamilyPreference
  codeFontFamily: FontFamilyPreference
} {
  return {
    uiFontFamily: normalizeFontFamilyPreference(settings.ui_font_family),
    codeFontFamily: normalizeFontFamilyPreference(settings.code_font_family),
  }
}

type AppearanceContextValue = {
  themeColor: ThemeColor
  setThemeColor: (color: ThemeColor) => void
  zoomLevel: ZoomLevel
  setZoomLevel: (zoom: ZoomLevel) => void
  uiFontFamily: FontFamilyPreference
  setUiFontFamily: (fontFamily: FontFamilyPreference) => void
  codeFontFamily: FontFamilyPreference
  setCodeFontFamily: (fontFamily: FontFamilyPreference) => void
  uiFontFamilyStack: string
  codeFontFamilyStack: string
}

export const AppearanceContext = createContext<AppearanceContextValue | null>(
  null
)

/**
 * AppearanceProvider 管理 themeColor 和 zoomLevel 两个外观偏好。
 *
 * 与 next-themes 完全正交：next-themes 负责 <html class="dark/light">，
 * 这里负责 <html data-theme="..."> 和 <html style="font-size: ...">。
 *
 * 注意：next-themes 的 attribute 配置必须保持 "class"。如果改为 "data-theme"
 * 会与本 Provider 冲突，导致主题色无法生效。
 */
export function AppearanceProvider({
  children,
}: {
  children: React.ReactNode
}) {
  // 初始值从 DOM 读取（appearance-script.ts 在 hydration 前已经写好），
  // 而不是从 localStorage 读 —— 避免 SSR 与 CSR 不一致导致的双闪烁。
  const [themeColor, setThemeColorState] = useState<ThemeColor>(() => {
    if (typeof document === "undefined") return DEFAULT_THEME_COLOR
    const attr = document.documentElement.getAttribute(
      "data-theme"
    ) as ThemeColor | null
    return attr && (THEME_COLORS as readonly string[]).includes(attr)
      ? attr
      : DEFAULT_THEME_COLOR
  })

  const [zoomLevel, setZoomLevelState] = useState<ZoomLevel>(() => {
    if (typeof document === "undefined") return DEFAULT_ZOOM_LEVEL
    const px = parseFloat(document.documentElement.style.fontSize || "16")
    const level = Math.round((px / 16) * 100) as ZoomLevel
    return (ZOOM_LEVELS as readonly number[]).includes(level)
      ? level
      : DEFAULT_ZOOM_LEVEL
  })

  const [uiFontFamily, setUiFontFamilyState] = useState<FontFamilyPreference>(
    () => readFontFamilyFromDataset("uiFontFamily")
  )

  const [codeFontFamily, setCodeFontFamilyState] =
    useState<FontFamilyPreference>(() =>
      readFontFamilyFromDataset("codeFontFamily")
    )

  const fontSettingsRef = useRef({
    uiFontFamily: normalizeFontFamilyPreference(uiFontFamily),
    codeFontFamily: normalizeFontFamilyPreference(codeFontFamily),
  })
  const desktopFontPersistenceReadyRef = useRef(false)
  const desktopFontSettingsDirtyRef = useRef(false)
  const desktopFontPersistTimerRef = useRef<number | null>(null)
  const desktopFontPersistQueueRef = useRef<Promise<void>>(Promise.resolve())

  const enqueueDesktopFontSettingsPersist = useCallback(() => {
    if (!isTauriDesktop()) return

    const settings = {
      uiFontFamily: fontSettingsRef.current.uiFontFamily,
      codeFontFamily: fontSettingsRef.current.codeFontFamily,
    }

    desktopFontPersistQueueRef.current = desktopFontPersistQueueRef.current
      .catch(() => {})
      .then(() =>
        updateSystemFontSettings({
          ui_font_family: settings.uiFontFamily,
          code_font_family: settings.codeFontFamily,
        }).then(() => undefined)
      )
  }, [])

  const scheduleDesktopFontSettingsPersist = useCallback(() => {
    if (!isTauriDesktop()) return
    if (desktopFontPersistTimerRef.current !== null) {
      window.clearTimeout(desktopFontPersistTimerRef.current)
    }
    desktopFontPersistTimerRef.current = window.setTimeout(() => {
      desktopFontPersistTimerRef.current = null
      enqueueDesktopFontSettingsPersist()
    }, 0)
  }, [enqueueDesktopFontSettingsPersist])

  const persistDesktopFontSettings = useCallback(
    (settings: {
      uiFontFamily: FontFamilyPreference
      codeFontFamily: FontFamilyPreference
    }) => {
      fontSettingsRef.current = {
        uiFontFamily: normalizeFontFamilyPreference(settings.uiFontFamily),
        codeFontFamily: normalizeFontFamilyPreference(settings.codeFontFamily),
      }

      if (!isTauriDesktop()) return
      if (!desktopFontPersistenceReadyRef.current) {
        desktopFontSettingsDirtyRef.current = true
        return
      }

      scheduleDesktopFontSettingsPersist()
    },
    [scheduleDesktopFontSettingsPersist]
  )

  const setThemeColor = useCallback((color: ThemeColor) => {
    setThemeColorState(color)
    document.documentElement.setAttribute("data-theme", color)
    try {
      localStorage.setItem(STORAGE_KEY_THEME_COLOR, color)
    } catch {
      // 隐私模式 / 禁用 storage 时静默忽略，本次会话内仍然生效
    }
  }, [])

  const setZoomLevel = useCallback((zoom: ZoomLevel) => {
    setZoomLevelState(zoom)
    document.documentElement.style.fontSize = `${(16 * zoom) / 100}px`
    syncTrafficLightPosition(zoom)
    try {
      localStorage.setItem(STORAGE_KEY_ZOOM_LEVEL, String(zoom))
    } catch {
      // 同上
    }
  }, [])

  const setUiFontFamily = useCallback(
    (fontFamily: FontFamilyPreference) => {
      const normalized = normalizeFontFamilyPreference(fontFamily)
      const nextSettings = {
        ...fontSettingsRef.current,
        uiFontFamily: normalized,
      }
      setUiFontFamilyState(normalized)
      applyUiFontFamily(normalized)
      writeFontFamilyToStorage(STORAGE_KEY_UI_FONT_FAMILY, normalized)
      persistDesktopFontSettings(nextSettings)
    },
    [persistDesktopFontSettings]
  )

  const setCodeFontFamily = useCallback(
    (fontFamily: FontFamilyPreference) => {
      const normalized = normalizeFontFamilyPreference(fontFamily)
      const nextSettings = {
        ...fontSettingsRef.current,
        codeFontFamily: normalized,
      }
      setCodeFontFamilyState(normalized)
      applyCodeFontFamily(normalized)
      writeFontFamilyToStorage(STORAGE_KEY_CODE_FONT_FAMILY, normalized)
      persistDesktopFontSettings(nextSettings)
    },
    [persistDesktopFontSettings]
  )

  const applyFontSettings = useCallback(
    (settings: {
      uiFontFamily: FontFamilyPreference
      codeFontFamily: FontFamilyPreference
    }) => {
      const normalizedUiFont = normalizeFontFamilyPreference(
        settings.uiFontFamily
      )
      const normalizedCodeFont = normalizeFontFamilyPreference(
        settings.codeFontFamily
      )
      fontSettingsRef.current = {
        uiFontFamily: normalizedUiFont,
        codeFontFamily: normalizedCodeFont,
      }
      setUiFontFamilyState(normalizedUiFont)
      setCodeFontFamilyState(normalizedCodeFont)
      applyUiFontFamily(normalizedUiFont)
      applyCodeFontFamily(normalizedCodeFont)
      writeFontFamilyToStorage(STORAGE_KEY_UI_FONT_FAMILY, normalizedUiFont)
      writeFontFamilyToStorage(STORAGE_KEY_CODE_FONT_FAMILY, normalizedCodeFont)
    },
    []
  )

  useEffect(() => {
    if (!isTauriDesktop()) return

    let cancelled = false

    Promise.all([getSystemFontSettings(), listSystemFontFamilies()])
      .then(([settings, fontList]) => {
        if (cancelled || desktopFontSettingsDirtyRef.current) return

        const currentStoredUiFont = readFontFamilyStorageState(
          STORAGE_KEY_UI_FONT_FAMILY
        )
        const currentStoredCodeFont = readFontFamilyStorageState(
          STORAGE_KEY_CODE_FONT_FAMILY
        )
        const dbSettings = normalizeSystemFontSettings(settings)
        const hasRestorableDbPreference =
          (!currentStoredUiFont.hasStoredValue &&
            dbSettings.uiFontFamily !== null) ||
          (!currentStoredCodeFont.hasStoredValue &&
            dbSettings.codeFontFamily !== null)

        if (!hasRestorableDbPreference) return

        const nextUiFont = currentStoredUiFont.hasStoredValue
          ? currentStoredUiFont.fontFamily
          : dbSettings.uiFontFamily
        const nextCodeFont = currentStoredCodeFont.hasStoredValue
          ? currentStoredCodeFont.fontFamily
          : dbSettings.codeFontFamily
        const validatedSettings = {
          uiFontFamily: isKnownFontFamily(nextUiFont, fontList.families)
            ? nextUiFont
            : DEFAULT_UI_FONT_FAMILY,
          codeFontFamily: isKnownCodeFontFamily(nextCodeFont, fontList.families)
            ? nextCodeFont
            : DEFAULT_CODE_FONT_FAMILY,
        }

        applyFontSettings(validatedSettings)
        desktopFontSettingsDirtyRef.current = false
        enqueueDesktopFontSettingsPersist()
      })
      .catch(() => {
        // Keep localStorage/current session values when desktop DB restore fails.
      })
      .finally(() => {
        if (!cancelled) {
          desktopFontPersistenceReadyRef.current = true
          if (desktopFontSettingsDirtyRef.current) {
            desktopFontSettingsDirtyRef.current = false
            scheduleDesktopFontSettingsPersist()
          }
        }
      })

    return () => {
      cancelled = true
      if (desktopFontPersistTimerRef.current !== null) {
        window.clearTimeout(desktopFontPersistTimerRef.current)
        desktopFontPersistTimerRef.current = null
      }
    }
  }, [
    applyFontSettings,
    enqueueDesktopFontSettingsPersist,
    scheduleDesktopFontSettingsPersist,
  ])

  useEffect(() => {
    let cancelled = false

    listSystemFontFamilies()
      .then((fontList) => {
        if (cancelled) return

        let storedUiFont: FontFamilyPreference = null
        let storedCodeFont: FontFamilyPreference = null
        try {
          storedUiFont = normalizeFontFamilyPreference(
            localStorage.getItem(STORAGE_KEY_UI_FONT_FAMILY)
          )
          storedCodeFont = normalizeFontFamilyPreference(
            localStorage.getItem(STORAGE_KEY_CODE_FONT_FAMILY)
          )
        } catch {
          return
        }

        if (!isKnownFontFamily(storedUiFont, fontList.families)) {
          setUiFontFamily(DEFAULT_UI_FONT_FAMILY)
        } else {
          fontSettingsRef.current = {
            ...fontSettingsRef.current,
            uiFontFamily: storedUiFont,
          }
        }
        if (!isKnownCodeFontFamily(storedCodeFont, fontList.families)) {
          setCodeFontFamily(DEFAULT_CODE_FONT_FAMILY)
        } else {
          fontSettingsRef.current = {
            ...fontSettingsRef.current,
            codeFontFamily: storedCodeFont,
          }
        }
      })
      .catch(() => {
        // Keep the current session value when the font-list API is unavailable.
      })

    return () => {
      cancelled = true
    }
  }, [setCodeFontFamily, setUiFontFamily])

  // Sync traffic-light position and appearance mode on mount
  useEffect(() => {
    syncTrafficLightPosition(zoomLevel)
    try {
      syncAppearanceMode(localStorage.getItem("theme") ?? "system")
    } catch {
      // localStorage unavailable
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // 跨标签页同步：用户在另一个窗口改了设置时，本窗口实时跟进
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === STORAGE_KEY_THEME_COLOR && e.newValue) {
        const color = e.newValue as ThemeColor
        if ((THEME_COLORS as readonly string[]).includes(color)) {
          setThemeColorState(color)
          document.documentElement.setAttribute("data-theme", color)
        }
      }
      if (e.key === STORAGE_KEY_ZOOM_LEVEL && e.newValue) {
        const zoom = parseInt(e.newValue, 10) as ZoomLevel
        if ((ZOOM_LEVELS as readonly number[]).includes(zoom)) {
          setZoomLevelState(zoom)
          document.documentElement.style.fontSize = `${(16 * zoom) / 100}px`
          syncTrafficLightPosition(zoom)
        }
      }
      if (e.key === STORAGE_KEY_UI_FONT_FAMILY) {
        const fontFamily = normalizeFontFamilyPreference(e.newValue)
        desktopFontSettingsDirtyRef.current = true
        fontSettingsRef.current = {
          ...fontSettingsRef.current,
          uiFontFamily: fontFamily,
        }
        setUiFontFamilyState(fontFamily)
        applyUiFontFamily(fontFamily)
      }
      if (e.key === STORAGE_KEY_CODE_FONT_FAMILY) {
        const fontFamily = normalizeFontFamilyPreference(e.newValue)
        desktopFontSettingsDirtyRef.current = true
        fontSettingsRef.current = {
          ...fontSettingsRef.current,
          codeFontFamily: fontFamily,
        }
        setCodeFontFamilyState(fontFamily)
        applyCodeFontFamily(fontFamily)
      }
      // Sync appearance mode to Tauri DB when changed in another window
      if (e.key === "theme") {
        syncAppearanceMode(e.newValue ?? "system")
      }
    }
    window.addEventListener("storage", onStorage)
    return () => window.removeEventListener("storage", onStorage)
  }, [])

  const uiFontFamilyStack = buildUiFontFamilyStack(uiFontFamily)
  const codeFontFamilyStack = buildCodeFontFamilyStack(codeFontFamily)

  return (
    <AppearanceContext.Provider
      value={{
        themeColor,
        setThemeColor,
        zoomLevel,
        setZoomLevel,
        uiFontFamily: uiFontFamily ?? DEFAULT_UI_FONT_FAMILY,
        setUiFontFamily,
        codeFontFamily: codeFontFamily ?? DEFAULT_CODE_FONT_FAMILY,
        setCodeFontFamily,
        uiFontFamilyStack,
        codeFontFamilyStack,
      }}
    >
      {children}
    </AppearanceContext.Provider>
  )
}
