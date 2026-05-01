"use client"

import {
  createContext,
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
} from "react"
import {
  getSystemFontSettings,
  listSystemFontFamilies,
  updateSystemFontSettings,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import {
  THEME_COLORS,
  DEFAULT_THEME_COLOR,
  DEFAULT_CODE_FONT_FAMILY,
  DEFAULT_UI_FONT_FAMILY,
  DEFAULT_ZOOM_LEVEL,
  FALLBACK_SYSTEM_FONT_FAMILY_LIST,
  ZOOM_LEVELS,
  buildCodeFontFamilyStack,
  buildUiFontFamilyStack,
  isKnownCodeFontFamily,
  isKnownFontFamily,
  normalizeFontFamilyPreference,
  normalizeSystemFontFamilyList,
  type FontFamilyPreference,
  type ThemeColor,
  type ZoomLevel,
} from "@/lib/theme-presets"
import {
  STORAGE_KEY_CODE_FONT_FAMILY,
  STORAGE_KEY_THEME_COLOR,
  STORAGE_KEY_UI_FONT_FAMILY,
  STORAGE_KEY_ZOOM_LEVEL,
} from "@/lib/appearance-script"
import type { SystemFontFamilyList, SystemFontSettings } from "@/lib/types"

const FONT_PERSIST_DELAY_MS = 200

function syncTrafficLightPosition(zoom: number) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return
  }

  import("@/lib/tauri").then((t) =>
    t.updateTrafficLightPosition(zoom).catch(() => {})
  )
}

function syncAppearanceMode(mode: string) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) {
    return
  }

  import("@/lib/tauri").then((t) =>
    t.updateAppearanceMode(mode).catch(() => {})
  )
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
    // Keep the in-session value when localStorage is unavailable.
  }
}

function readFontFamilyStorageState(key: string): {
  hasStoredValue: boolean
  fontFamily: FontFamilyPreference
} {
  if (typeof window === "undefined") {
    return { hasStoredValue: false, fontFamily: null }
  }

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

function normalizeSystemFontSettings(settings: SystemFontSettings): {
  uiFontFamily: FontFamilyPreference
  codeFontFamily: FontFamilyPreference
} {
  return {
    uiFontFamily: normalizeFontFamilyPreference(settings.ui_font_family),
    codeFontFamily: normalizeFontFamilyPreference(settings.code_font_family),
  }
}

function resolveUiFontFamily(
  fontFamily: FontFamilyPreference,
  fontList: SystemFontFamilyList
): FontFamilyPreference {
  const normalized = normalizeFontFamilyPreference(fontFamily)
  if (!normalized) {
    return DEFAULT_UI_FONT_FAMILY
  }
  if (fontList.source === "fallback") {
    return normalized
  }
  return isKnownFontFamily(normalized, fontList.families)
    ? normalized
    : DEFAULT_UI_FONT_FAMILY
}

function resolveCodeFontFamily(
  fontFamily: FontFamilyPreference,
  fontList: SystemFontFamilyList
): FontFamilyPreference {
  const normalized = normalizeFontFamilyPreference(fontFamily)
  if (!normalized) {
    return DEFAULT_CODE_FONT_FAMILY
  }
  if (fontList.source === "fallback") {
    return normalized
  }
  return isKnownCodeFontFamily(normalized, fontList.families)
    ? normalized
    : DEFAULT_CODE_FONT_FAMILY
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
  fontList: SystemFontFamilyList
  fontListLoaded: boolean
  fontListError: string | null
}

export const AppearanceContext = createContext<AppearanceContextValue | null>(
  null
)

/**
 * AppearanceProvider 管理 themeColor、zoomLevel、UI 字体和代码字体。
 *
 * 与 next-themes 完全正交：next-themes 负责 <html class="dark/light">，
 * 这里负责 <html data-theme="...">、<html style="font-size: ..."> 和字体 CSS 变量。
 */
export function AppearanceProvider({
  children,
}: {
  children: React.ReactNode
}) {
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
  const [fontList, setFontList] = useState<SystemFontFamilyList>(() =>
    normalizeSystemFontFamilyList(FALLBACK_SYSTEM_FONT_FAMILY_LIST)
  )
  const [fontListLoaded, setFontListLoaded] = useState(false)
  const [fontListError, setFontListError] = useState<string | null>(null)

  const persistTimerRef = useRef<number | null>(null)
  const fontSettingsRef = useRef({
    uiFontFamily: normalizeFontFamilyPreference(uiFontFamily),
    codeFontFamily: normalizeFontFamilyPreference(codeFontFamily),
  })

  const applyFontSettings = useCallback(
    (settings: {
      uiFontFamily: FontFamilyPreference
      codeFontFamily: FontFamilyPreference
      writeStorage: boolean
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

      if (settings.writeStorage) {
        writeFontFamilyToStorage(STORAGE_KEY_UI_FONT_FAMILY, normalizedUiFont)
        writeFontFamilyToStorage(
          STORAGE_KEY_CODE_FONT_FAMILY,
          normalizedCodeFont
        )
      }
    },
    []
  )

  const schedulePersistFontSettings = useCallback(() => {
    if (typeof window === "undefined") return

    if (persistTimerRef.current !== null) {
      window.clearTimeout(persistTimerRef.current)
    }

    persistTimerRef.current = window.setTimeout(() => {
      persistTimerRef.current = null
      const settings = fontSettingsRef.current
      updateSystemFontSettings({
        ui_font_family: settings.uiFontFamily,
        code_font_family: settings.codeFontFamily,
      }).catch(() => {
        // Keep localStorage/current session values when backend persistence fails.
      })
    }, FONT_PERSIST_DELAY_MS)
  }, [])

  const setThemeColor = useCallback((color: ThemeColor) => {
    setThemeColorState(color)
    document.documentElement.setAttribute("data-theme", color)

    try {
      localStorage.setItem(STORAGE_KEY_THEME_COLOR, color)
    } catch {
      // Keep the in-session value when localStorage is unavailable.
    }
  }, [])

  const setZoomLevel = useCallback((zoom: ZoomLevel) => {
    setZoomLevelState(zoom)
    document.documentElement.style.fontSize = `${(16 * zoom) / 100}px`
    syncTrafficLightPosition(zoom)

    try {
      localStorage.setItem(STORAGE_KEY_ZOOM_LEVEL, String(zoom))
    } catch {
      // Keep the in-session value when localStorage is unavailable.
    }
  }, [])

  const setUiFontFamily = useCallback(
    (fontFamily: FontFamilyPreference) => {
      applyFontSettings({
        uiFontFamily: fontFamily,
        codeFontFamily: fontSettingsRef.current.codeFontFamily,
        writeStorage: true,
      })
      schedulePersistFontSettings()
    },
    [applyFontSettings, schedulePersistFontSettings]
  )

  const setCodeFontFamily = useCallback(
    (fontFamily: FontFamilyPreference) => {
      applyFontSettings({
        uiFontFamily: fontSettingsRef.current.uiFontFamily,
        codeFontFamily: fontFamily,
        writeStorage: true,
      })
      schedulePersistFontSettings()
    },
    [applyFontSettings, schedulePersistFontSettings]
  )

  useEffect(() => {
    let cancelled = false

    const bootstrapFonts = async () => {
      const settings = await getSystemFontSettings().catch(
        (): SystemFontSettings => ({
          ui_font_family: null,
          code_font_family: null,
        })
      )

      try {
        const loadedFontList = await listSystemFontFamilies()
        if (cancelled) return

        const normalizedFontList = normalizeSystemFontFamilyList(loadedFontList)
        setFontList(normalizedFontList)
        setFontListError(null)

        const currentStoredUiFont = readFontFamilyStorageState(
          STORAGE_KEY_UI_FONT_FAMILY
        )
        const currentStoredCodeFont = readFontFamilyStorageState(
          STORAGE_KEY_CODE_FONT_FAMILY
        )
        const persistedSettings = normalizeSystemFontSettings(settings)

        const nextUiFont = currentStoredUiFont.hasStoredValue
          ? currentStoredUiFont.fontFamily
          : persistedSettings.uiFontFamily
        const nextCodeFont = currentStoredCodeFont.hasStoredValue
          ? currentStoredCodeFont.fontFamily
          : persistedSettings.codeFontFamily

        applyFontSettings({
          uiFontFamily: resolveUiFontFamily(nextUiFont, normalizedFontList),
          codeFontFamily: resolveCodeFontFamily(
            nextCodeFont,
            normalizedFontList
          ),
          writeStorage: true,
        })
      } catch (error) {
        if (cancelled) return

        const fallbackFontList = normalizeSystemFontFamilyList(
          FALLBACK_SYSTEM_FONT_FAMILY_LIST
        )
        setFontList(fallbackFontList)
        setFontListError(toErrorMessage(error))

        const currentStoredUiFont = readFontFamilyStorageState(
          STORAGE_KEY_UI_FONT_FAMILY
        )
        const currentStoredCodeFont = readFontFamilyStorageState(
          STORAGE_KEY_CODE_FONT_FAMILY
        )
        const persistedSettings = normalizeSystemFontSettings(settings)
        const nextUiFont = currentStoredUiFont.hasStoredValue
          ? currentStoredUiFont.fontFamily
          : persistedSettings.uiFontFamily
        const nextCodeFont = currentStoredCodeFont.hasStoredValue
          ? currentStoredCodeFont.fontFamily
          : persistedSettings.codeFontFamily

        applyFontSettings({
          uiFontFamily: resolveUiFontFamily(nextUiFont, fallbackFontList),
          codeFontFamily: resolveCodeFontFamily(nextCodeFont, fallbackFontList),
          writeStorage: true,
        })
      } finally {
        if (!cancelled) {
          setFontListLoaded(true)
        }
      }
    }

    void bootstrapFonts()

    return () => {
      cancelled = true
      if (persistTimerRef.current !== null) {
        window.clearTimeout(persistTimerRef.current)
        persistTimerRef.current = null
      }
    }
  }, [applyFontSettings])

  useEffect(() => {
    syncTrafficLightPosition(zoomLevel)

    try {
      syncAppearanceMode(localStorage.getItem("theme") ?? "system")
    } catch {
      // localStorage unavailable
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

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
        applyFontSettings({
          uiFontFamily: e.newValue,
          codeFontFamily: fontSettingsRef.current.codeFontFamily,
          writeStorage: false,
        })
      }

      if (e.key === STORAGE_KEY_CODE_FONT_FAMILY) {
        applyFontSettings({
          uiFontFamily: fontSettingsRef.current.uiFontFamily,
          codeFontFamily: e.newValue,
          writeStorage: false,
        })
      }

      if (e.key === "theme") {
        syncAppearanceMode(e.newValue ?? "system")
      }
    }

    window.addEventListener("storage", onStorage)
    return () => window.removeEventListener("storage", onStorage)
  }, [applyFontSettings])

  const value = useMemo(
    () => ({
      themeColor,
      setThemeColor,
      zoomLevel,
      setZoomLevel,
      uiFontFamily,
      setUiFontFamily,
      codeFontFamily,
      setCodeFontFamily,
      uiFontFamilyStack: buildUiFontFamilyStack(uiFontFamily),
      codeFontFamilyStack: buildCodeFontFamilyStack(codeFontFamily),
      fontList,
      fontListLoaded,
      fontListError,
    }),
    [
      codeFontFamily,
      fontList,
      fontListError,
      fontListLoaded,
      setCodeFontFamily,
      setThemeColor,
      setUiFontFamily,
      setZoomLevel,
      themeColor,
      uiFontFamily,
      zoomLevel,
    ]
  )

  return (
    <AppearanceContext.Provider value={value}>
      {children}
    </AppearanceContext.Provider>
  )
}
