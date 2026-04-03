"use client"

import {
  Check,
  Monitor,
  Moon,
  Minus,
  Plus,
  Sun,
  Type,
  Palette,
  Terminal,
  Accessibility,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { useTheme } from "next-themes"
import { cn } from "@/lib/utils"
import { useAppearance } from "@/lib/appearance/use-appearance"
import {
  ACCENT_PRESETS,
  UI_FONT_PRESETS,
  CODE_FONT_PRESETS,
  CODE_THEME_LIGHT_OPTIONS,
  CODE_THEME_DARK_OPTIONS,
  TERMINAL_SCHEME_OPTIONS,
} from "@/lib/appearance/constants"
import type {
  AccentColor,
  AppearanceSettings,
  CodeThemeLight,
  CodeThemeDark,
  TerminalScheme,
  InterfaceDensity,
} from "@/lib/appearance/types"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"

type ThemeMode = "system" | "light" | "dark"

function FontSizeInput({
  value,
  onChange,
  min,
  max,
}: {
  value: number
  onChange: (v: number) => void
  min: number
  max: number
}) {
  return (
    <div className="flex items-center gap-2">
      <button
        type="button"
        className="flex h-7 w-7 items-center justify-center rounded-md border text-muted-foreground hover:bg-muted disabled:opacity-50"
        disabled={value <= min}
        onClick={() => onChange(Math.max(min, value - 1))}
      >
        <Minus className="h-3 w-3" />
      </button>
      <span className="w-10 text-center text-sm tabular-nums">{value}px</span>
      <button
        type="button"
        className="flex h-7 w-7 items-center justify-center rounded-md border text-muted-foreground hover:bg-muted disabled:opacity-50"
        disabled={value >= max}
        onClick={() => onChange(Math.min(max, value + 1))}
      >
        <Plus className="h-3 w-3" />
      </button>
    </div>
  )
}

export function AppearanceSettings() {
  const t = useTranslations("AppearanceSettings")
  const { theme, resolvedTheme, setTheme } = useTheme()
  const { settings, update } = useAppearance()

  const resolvedThemeLabel =
    resolvedTheme === "dark"
      ? t("resolvedTheme.dark")
      : resolvedTheme === "light"
        ? t("resolvedTheme.light")
        : t("resolvedTheme.unknown")

  return (
    <div className="h-full overflow-auto">
      <div className="w-full space-y-4">
        {/* Section 1: Theme */}
        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Sun className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("sectionTitle")}</h2>
          </div>
          <p className="text-xs text-muted-foreground leading-5">
            {t("sectionDescription")}
          </p>

          {/* Theme mode */}
          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("themeMode")}
            </label>
            <Select
              value={theme ?? "system"}
              onValueChange={(v) => setTheme(v as ThemeMode)}
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

          {/* Accent color */}
          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("accentColor")}
            </label>
            <div className="flex flex-wrap gap-2">
              {(Object.keys(ACCENT_PRESETS) as AccentColor[]).map((color) => (
                <button
                  key={color}
                  type="button"
                  title={ACCENT_PRESETS[color].label}
                  className={cn(
                    "flex h-7 w-7 items-center justify-center rounded-full border-2 transition-transform hover:scale-110",
                    settings.accentColor === color
                      ? "border-foreground"
                      : "border-transparent"
                  )}
                  onClick={() => update("accentColor", color)}
                >
                  <span
                    className="flex h-5 w-5 items-center justify-center rounded-full"
                    style={{ backgroundColor: ACCENT_PRESETS[color].dot }}
                  >
                    {settings.accentColor === color && (
                      <Check className="h-3 w-3 text-white" />
                    )}
                  </span>
                </button>
              ))}
            </div>
          </div>
        </section>

        {/* Section 2: Fonts */}
        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Type className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("fontSection")}</h2>
          </div>
          <p className="text-xs text-muted-foreground leading-5">
            {t("fontSectionDesc")}
          </p>

          {/* UI Font */}
          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("uiFont")}
            </label>
            <Select
              value={settings.uiFont}
              onValueChange={(v) => update("uiFont", v)}
            >
              <SelectTrigger className="w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                {UI_FONT_PRESETS.map((p) => (
                  <SelectItem key={p.value} value={p.value}>
                    {p.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          {/* Code Font */}
          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("codeFont")}
            </label>
            <Select
              value={settings.codeFont}
              onValueChange={(v) => update("codeFont", v)}
            >
              <SelectTrigger className="w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                {CODE_FONT_PRESETS.map((p) => (
                  <SelectItem key={p.value} value={p.value}>
                    {p.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>

          {/* Font Sizes */}
          <div className="flex gap-8">
            <div className="space-y-2">
              <label className="text-xs font-medium text-muted-foreground">
                {t("uiFontSize")}
              </label>
              <FontSizeInput
                value={settings.uiFontSize}
                min={12}
                max={18}
                onChange={(v) => update("uiFontSize", v)}
              />
            </div>
            <div className="space-y-2">
              <label className="text-xs font-medium text-muted-foreground">
                {t("codeFontSize")}
              </label>
              <FontSizeInput
                value={settings.codeFontSize}
                min={12}
                max={20}
                onChange={(v) => update("codeFontSize", v)}
              />
            </div>
          </div>
        </section>

        {/* Section 3: Editor */}
        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Palette className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("editorSection")}</h2>
          </div>
          <p className="text-xs text-muted-foreground leading-5">
            {t("editorSectionDesc")}
          </p>

          <div className="flex gap-8">
            <div className="space-y-2">
              <label className="text-xs font-medium text-muted-foreground">
                {t("codeThemeLight")}
              </label>
              <Select
                value={settings.codeThemeLight}
                onValueChange={(v) =>
                  update("codeThemeLight", v as CodeThemeLight)
                }
              >
                <SelectTrigger className="w-44">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent align="start">
                  {CODE_THEME_LIGHT_OPTIONS.map((o) => (
                    <SelectItem key={o.value} value={o.value}>
                      {o.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
            <div className="space-y-2">
              <label className="text-xs font-medium text-muted-foreground">
                {t("codeThemeDark")}
              </label>
              <Select
                value={settings.codeThemeDark}
                onValueChange={(v) =>
                  update("codeThemeDark", v as CodeThemeDark)
                }
              >
                <SelectTrigger className="w-44">
                  <SelectValue />
                </SelectTrigger>
                <SelectContent align="start">
                  {CODE_THEME_DARK_OPTIONS.map((o) => (
                    <SelectItem key={o.value} value={o.value}>
                      {o.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>
          </div>
        </section>

        {/* Section 4: Terminal */}
        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Terminal className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("terminalSection")}</h2>
          </div>
          <p className="text-xs text-muted-foreground leading-5">
            {t("terminalSectionDesc")}
          </p>

          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("terminalScheme")}
            </label>
            <Select
              value={settings.terminalScheme}
              onValueChange={(v) =>
                update("terminalScheme", v as TerminalScheme)
              }
            >
              <SelectTrigger className="w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                {TERMINAL_SCHEME_OPTIONS.map((o) => (
                  <SelectItem key={o.value} value={o.value}>
                    {o.label}
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
          </div>
        </section>

        {/* Section 5: Accessibility */}
        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Accessibility className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">
              {t("accessibilitySection")}
            </h2>
          </div>
          <p className="text-xs text-muted-foreground leading-5">
            {t("accessibilitySectionDesc")}
          </p>

          {/* Density */}
          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("density")}
            </label>
            <Select
              value={settings.density}
              onValueChange={(v) => update("density", v as InterfaceDensity)}
            >
              <SelectTrigger className="w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                <SelectItem value="compact">{t("densityCompact")}</SelectItem>
                <SelectItem value="default">{t("densityDefault")}</SelectItem>
                <SelectItem value="spacious">{t("densitySpacious")}</SelectItem>
              </SelectContent>
            </Select>
          </div>

          {/* Reduce Motion */}
          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("reduceMotion")}
            </label>
            <Select
              value={settings.reduceMotion}
              onValueChange={(v) =>
                update("reduceMotion", v as AppearanceSettings["reduceMotion"])
              }
            >
              <SelectTrigger className="w-56">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                <SelectItem value="system">
                  {t("reduceMotionSystem")}
                </SelectItem>
                <SelectItem value="on">{t("reduceMotionOn")}</SelectItem>
                <SelectItem value="off">{t("reduceMotionOff")}</SelectItem>
              </SelectContent>
            </Select>
          </div>
        </section>
      </div>
    </div>
  )
}
