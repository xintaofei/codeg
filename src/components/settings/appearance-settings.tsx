"use client"

import { Check, Monitor, Moon, Palette, Sun, Type } from "lucide-react"
import { useTranslations } from "next-intl"
import { useTheme } from "next-themes"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Slider } from "@/components/ui/slider"
import { Button } from "@/components/ui/button"
import { useAppearanceSettings } from "@/hooks/use-appearance-settings"
import {
  UI_FONT_SIZE_MIN,
  UI_FONT_SIZE_MAX,
  CODE_FONT_SIZE_MIN,
  CODE_FONT_SIZE_MAX,
  THEME_COLORS,
} from "@/lib/appearance-settings"
import { THEME_COLOR_PRESETS } from "@/lib/theme-color-presets"
import { cn } from "@/lib/utils"

type ThemeMode = "system" | "light" | "dark"

export function AppearanceSettings() {
  const t = useTranslations("AppearanceSettings")
  const { theme, resolvedTheme, setTheme } = useTheme()
  const {
    appearance,
    updateThemeColor,
    updateUiFontSize,
    updateCodeFontSize,
    resetAppearance,
  } = useAppearanceSettings()

  const resolvedThemeLabel =
    resolvedTheme === "dark"
      ? t("resolvedTheme.dark")
      : resolvedTheme === "light"
        ? t("resolvedTheme.light")
        : t("resolvedTheme.unknown")

  return (
    <div className="h-full overflow-auto">
      <div className="w-full space-y-4">
        {/* Section 1: Theme Mode */}
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
              onValueChange={(value) => setTheme(value as ThemeMode)}
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

        {/* Section 2: Theme Color */}
        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Palette className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("themeColor")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("themeColorDescription")}
          </p>

          <div className="flex flex-wrap gap-3">
            {THEME_COLORS.map((color) => {
              const preset = THEME_COLOR_PRESETS.find((p) => p.name === color)
              if (!preset) return null
              const isActive = appearance.themeColor === color
              return (
                <button
                  key={color}
                  type="button"
                  className="flex flex-col items-center gap-1.5"
                  onClick={() => updateThemeColor(color)}
                >
                  <div
                    className={cn(
                      "flex h-8 w-8 items-center justify-center rounded-full border-2 transition-colors",
                      isActive
                        ? "border-foreground"
                        : "border-transparent hover:border-muted-foreground/50"
                    )}
                    style={{ backgroundColor: preset.preview }}
                  >
                    {isActive && (
                      <Check className="h-4 w-4 text-white drop-shadow-[0_1px_2px_rgba(0,0,0,0.5)]" />
                    )}
                  </div>
                  <span
                    className={cn(
                      "text-[10px]",
                      isActive
                        ? "text-foreground font-medium"
                        : "text-muted-foreground"
                    )}
                  >
                    {t(`themeColors.${color}`)}
                  </span>
                </button>
              )
            })}
          </div>
        </section>

        {/* Section 3: Font Size */}
        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Type className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("fontSizeTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("fontSizeDescription")}
          </p>

          <div className="space-y-5">
            {/* UI Font Size */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <label className="text-xs font-medium text-muted-foreground">
                  {t("uiFontSize")}
                </label>
                <span className="text-xs font-semibold tabular-nums">
                  {appearance.uiFontSize}px
                </span>
              </div>
              <div className="flex items-center gap-3">
                <span className="text-[10px] text-muted-foreground w-6 text-right">
                  {UI_FONT_SIZE_MIN}
                </span>
                <Slider
                  min={UI_FONT_SIZE_MIN}
                  max={UI_FONT_SIZE_MAX}
                  step={1}
                  value={[appearance.uiFontSize]}
                  onValueChange={([v]) => updateUiFontSize(v)}
                  className="flex-1"
                />
                <span className="text-[10px] text-muted-foreground w-6">
                  {UI_FONT_SIZE_MAX}
                </span>
              </div>
            </div>

            {/* Code Font Size */}
            <div className="space-y-2">
              <div className="flex items-center justify-between">
                <label className="text-xs font-medium text-muted-foreground">
                  {t("codeFontSize")}
                </label>
                <span className="text-xs font-semibold tabular-nums">
                  {appearance.codeFontSize}px
                </span>
              </div>
              <div className="flex items-center gap-3">
                <span className="text-[10px] text-muted-foreground w-6 text-right">
                  {CODE_FONT_SIZE_MIN}
                </span>
                <Slider
                  min={CODE_FONT_SIZE_MIN}
                  max={CODE_FONT_SIZE_MAX}
                  step={1}
                  value={[appearance.codeFontSize]}
                  onValueChange={([v]) => updateCodeFontSize(v)}
                  className="flex-1"
                />
                <span className="text-[10px] text-muted-foreground w-6">
                  {CODE_FONT_SIZE_MAX}
                </span>
              </div>
            </div>
          </div>

          <Button
            variant="outline"
            size="sm"
            className="text-xs"
            onClick={resetAppearance}
          >
            {t("resetDefaults")}
          </Button>
        </section>
      </div>
    </div>
  )
}
