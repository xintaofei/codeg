"use client"

import { useEffect, useMemo } from "react"
import { Monitor, Moon, RotateCcw, Sun, Type } from "lucide-react"
import { useTranslations } from "next-intl"
import { useTheme } from "next-themes"
import { Button } from "@/components/ui/button"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  useAppearanceFontList,
  useCodeFontFamily,
  useThemeColor,
  useUiFontFamily,
  useZoomLevel,
} from "@/hooks/use-appearance"
import { cn } from "@/lib/utils"
import {
  buildCodeFontOptions,
  DEFAULT_CODE_FONT_FAMILY,
  DEFAULT_THEME_COLOR,
  DEFAULT_UI_FONT_FAMILY,
  DEFAULT_ZOOM_LEVEL,
  isKnownCodeFontFamily,
  isKnownFontFamily,
  THEME_COLOR_PREVIEW,
  THEME_COLORS,
  ZOOM_LEVELS,
  normalizeFontFamilyPreference,
  type FontFamilyPreference,
  type ThemeColor,
  type ZoomLevel,
} from "@/lib/theme-presets"

type ThemeMode = "system" | "light" | "dark"
type FontSelectValue = string

const DEFAULT_FONT_SELECT_VALUE = "__default__"

function toSelectValue(fontFamily: FontFamilyPreference): FontSelectValue {
  return fontFamily ?? DEFAULT_FONT_SELECT_VALUE
}

function fromSelectValue(value: FontSelectValue): FontFamilyPreference {
  return value === DEFAULT_FONT_SELECT_VALUE
    ? null
    : normalizeFontFamilyPreference(value)
}

export function AppearanceSettings() {
  const t = useTranslations("AppearanceSettings")
  const { theme, resolvedTheme, setTheme } = useTheme()
  const { themeColor, setThemeColor } = useThemeColor()
  const { zoomLevel, setZoomLevel } = useZoomLevel()
  const { uiFontFamily, setUiFontFamily } = useUiFontFamily()
  const { codeFontFamily, setCodeFontFamily } = useCodeFontFamily()
  const { fontList, fontListLoaded, fontListError } = useAppearanceFontList()

  const uiFontOptions = fontList.families
  const codeFontOptions = useMemo(
    () => buildCodeFontOptions(fontList.families),
    [fontList.families]
  )

  useEffect(() => {
    if (!fontListLoaded || fontListError || fontList.source === "fallback") {
      return
    }
    if (!isKnownFontFamily(uiFontFamily, uiFontOptions)) {
      setUiFontFamily(DEFAULT_UI_FONT_FAMILY)
    }
  }, [
    fontList.source,
    fontListError,
    fontListLoaded,
    setUiFontFamily,
    uiFontFamily,
    uiFontOptions,
  ])

  useEffect(() => {
    if (!fontListLoaded || fontListError || fontList.source === "fallback") {
      return
    }
    if (!isKnownCodeFontFamily(codeFontFamily, codeFontOptions)) {
      setCodeFontFamily(DEFAULT_CODE_FONT_FAMILY)
    }
  }, [
    codeFontFamily,
    codeFontOptions,
    fontList.source,
    fontListError,
    fontListLoaded,
    setCodeFontFamily,
  ])

  const resolvedThemeLabel =
    resolvedTheme === "dark"
      ? t("resolvedTheme.dark")
      : resolvedTheme === "light"
        ? t("resolvedTheme.light")
        : t("resolvedTheme.unknown")

  const isAtDefaults =
    themeColor === DEFAULT_THEME_COLOR &&
    zoomLevel === DEFAULT_ZOOM_LEVEL &&
    uiFontFamily === DEFAULT_UI_FONT_FAMILY &&
    codeFontFamily === DEFAULT_CODE_FONT_FAMILY

  const handleResetToDefaults = () => {
    setThemeColor(DEFAULT_THEME_COLOR)
    setZoomLevel(DEFAULT_ZOOM_LEVEL)
    setUiFontFamily(DEFAULT_UI_FONT_FAMILY)
    setCodeFontFamily(DEFAULT_CODE_FONT_FAMILY)
  }

  return (
    <ScrollArea className="h-full">
      <div className="w-full space-y-4 p-3 md:p-4">
        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Sun className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("sectionTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("sectionDescription")}
          </p>

          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("themeMode")}
            </label>
            <Select
              value={theme ?? "system"}
              onValueChange={(value) => {
                setTheme(value as ThemeMode)
                if (
                  typeof window !== "undefined" &&
                  "__TAURI_INTERNALS__" in window
                ) {
                  import("@/lib/tauri").then((tauriApi) =>
                    tauriApi.updateAppearanceMode(value).catch(() => {})
                  )
                }
              }}
            >
              <SelectTrigger className="w-56">
                <SelectValue placeholder={t("placeholder")} />
              </SelectTrigger>
              <SelectContent align="start">
                <SelectItem value="system">
                  <span className="inline-flex items-center gap-2">
                    <Monitor className="h-3.5 w-3.5" />
                    {t("system")}
                  </span>
                </SelectItem>
                <SelectItem value="light">
                  <span className="inline-flex items-center gap-2">
                    <Sun className="h-3.5 w-3.5" />
                    {t("light")}
                  </span>
                </SelectItem>
                <SelectItem value="dark">
                  <span className="inline-flex items-center gap-2">
                    <Moon className="h-3.5 w-3.5" />
                    {t("dark")}
                  </span>
                </SelectItem>
              </SelectContent>
            </Select>
            <p
              className="text-[11px] text-muted-foreground"
              suppressHydrationWarning
            >
              {t("currentTheme", { theme: resolvedThemeLabel })}
            </p>
          </div>
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <span
              className="size-4 rounded-full border"
              style={{ backgroundColor: THEME_COLOR_PREVIEW[themeColor] }}
              aria-hidden
            />
            <h2 className="text-sm font-semibold">
              {t("themeColor.sectionTitle")}
            </h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("themeColor.sectionDescription")}
          </p>

          <div className="grid grid-cols-3 gap-2 sm:grid-cols-4 md:grid-cols-6">
            {THEME_COLORS.map((color) => {
              const isActive = themeColor === color
              return (
                <button
                  key={color}
                  type="button"
                  onClick={() => setThemeColor(color as ThemeColor)}
                  aria-pressed={isActive}
                  className={cn(
                    "flex items-center gap-2 rounded-md border px-3 py-2 text-xs transition-colors",
                    "hover:bg-accent hover:text-accent-foreground",
                    isActive && "border-primary ring-2 ring-primary/30"
                  )}
                >
                  <span
                    className="size-4 shrink-0 rounded-full border"
                    style={{ backgroundColor: THEME_COLOR_PREVIEW[color] }}
                    aria-hidden
                  />
                  <span className="truncate">
                    {t(`themeColor.options.${color}`)}
                  </span>
                </button>
              )
            })}
          </div>

          <p className="text-[11px] text-muted-foreground">
            {t("themeColor.current", {
              color: t(`themeColor.options.${themeColor}`),
            })}
          </p>
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Type className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">
              {t("fontFamily.sectionTitle")}
            </h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("fontFamily.sectionDescription")}
          </p>

          <div className="space-y-3">
            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <label className="text-xs font-medium text-muted-foreground">
                {t("fontFamily.uiFont")}
              </label>
              <Select
                value={toSelectValue(uiFontFamily)}
                onValueChange={(value) =>
                  setUiFontFamily(fromSelectValue(value))
                }
              >
                <SelectTrigger className="w-full sm:w-64">
                  <SelectValue placeholder={t("fontFamily.placeholder")} />
                </SelectTrigger>
                <SelectContent align="end" className="max-h-72">
                  <SelectItem value={DEFAULT_FONT_SELECT_VALUE}>
                    {t("fontFamily.default")}
                  </SelectItem>
                  {uiFontOptions.map((font) => (
                    <SelectItem key={font.family} value={font.family}>
                      {font.family}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            <div className="flex flex-col gap-2 sm:flex-row sm:items-center sm:justify-between">
              <label className="text-xs font-medium text-muted-foreground">
                {t("fontFamily.codeFont")}
              </label>
              <Select
                value={toSelectValue(codeFontFamily)}
                onValueChange={(value) =>
                  setCodeFontFamily(fromSelectValue(value))
                }
              >
                <SelectTrigger className="w-full sm:w-64">
                  <SelectValue placeholder={t("fontFamily.placeholder")} />
                </SelectTrigger>
                <SelectContent align="end" className="max-h-72">
                  <SelectItem value={DEFAULT_FONT_SELECT_VALUE}>
                    {t("fontFamily.default")}
                  </SelectItem>
                  {codeFontOptions.map((font) => (
                    <SelectItem key={font.family} value={font.family}>
                      {font.family}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>

          {fontList.source === "fallback" && !fontListError && (
            <p className="text-[11px] leading-5 text-muted-foreground">
              {t("fontFamily.systemFallbackHint")}
            </p>
          )}
          {fontListError && (
            <p className="text-[11px] text-muted-foreground">
              {t("fontFamily.loadFailed", { message: fontListError })}
            </p>
          )}
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Type className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">
              {t("zoomLevel.sectionTitle")}
            </h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("zoomLevel.sectionDescription")}
          </p>

          <div className="space-y-2">
            <Select
              value={String(zoomLevel)}
              onValueChange={(value) =>
                setZoomLevel(parseInt(value, 10) as ZoomLevel)
              }
            >
              <SelectTrigger className="w-56">
                <SelectValue placeholder={t("zoomLevel.placeholder")} />
              </SelectTrigger>
              <SelectContent align="start">
                {ZOOM_LEVELS.map((z) => (
                  <SelectItem key={z} value={String(z)}>
                    {z}%
                    {z === DEFAULT_ZOOM_LEVEL
                      ? ` (${t("zoomLevel.default")})`
                      : ""}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            <p className="text-[11px] text-muted-foreground">
              {t("zoomLevel.current", { zoom: zoomLevel })}
            </p>
          </div>
        </section>

        <div className="flex justify-end">
          <Button
            variant="outline"
            size="sm"
            disabled={isAtDefaults}
            onClick={handleResetToDefaults}
            title={t("resetHint")}
          >
            <RotateCcw className="mr-2 h-3.5 w-3.5" />
            {t("resetToDefaults")}
          </Button>
        </div>
      </div>
    </ScrollArea>
  )
}
