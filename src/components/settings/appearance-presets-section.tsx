"use client"

import { useCallback, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { useTheme } from "next-themes"
import { toast } from "sonner"
import { Download, SwatchBook, Upload } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { useAppearancePresets, useCustomStyle } from "@/hooks/use-appearance"
import {
  BUNDLED_PRESETS,
  isBundledPresetId,
} from "@/lib/appearance-presets-bundled"
import {
  DEFAULT_CHAT_FONT_SIZE_REM,
  MAX_PRESET_BYTES,
  PRESET_MODES,
  findMatchingPreset,
  parseAppearancePreset,
  parseCssLength,
  presetFileName,
  presetMatches,
  serializeAppearancePreset,
  snapshotToPreset,
  spacingMultiplierFromToken,
  spacingToken,
  type AppearanceCurrent,
  type AppearancePreset,
  type PresetIssue,
  type PresetMode,
} from "@/lib/appearance-preset"
import { CODE_THEMES, codeThemeLabel, isCodeThemeId } from "@/lib/code-themes"
import { FONT_BY_ID } from "@/lib/font-presets"
import { saveTextFile } from "@/lib/save-file"
import { THEME_COLOR_PREVIEW, THEME_COLOR_TITLE } from "@/lib/theme-presets"
import { cn } from "@/lib/utils"

/** The density rungs; `1` is the stock layout and clears the token. */
const DENSITY_OPTIONS = [
  { key: "compact", value: 0.85 },
  { key: "snug", value: 0.92 },
  { key: "default", value: 1 },
  { key: "relaxed", value: 1.1 },
] as const

/** Message text rungs in rem; the default clears the token. */
const CHAT_TEXT_OPTIONS = [
  { key: "small", value: 0.8125 },
  { key: "default", value: DEFAULT_CHAT_FONT_SIZE_REM },
  { key: "large", value: 0.9375 },
  { key: "larger", value: 1 },
] as const

const CUSTOM_OPTION = "custom"

/** Files are `.json`; the suffix in the export name is a convention, not a gate. */
const PRESET_ACCEPT = ".json,application/json"

const LIGHT_CODE_THEMES = CODE_THEMES.filter((t) => t.type === "light")
const DARK_CODE_THEMES = CODE_THEMES.filter((t) => t.type === "dark")

type ImportState =
  | { kind: "preview"; preset: AppearancePreset; warnings: PresetIssue[] }
  | { kind: "errors"; issues: PresetIssue[] }
  | null

function isPresetMode(value: string | undefined): value is PresetMode {
  return !!value && (PRESET_MODES as readonly string[]).includes(value)
}

/**
 * Four swatches that stand for a preset in the gallery, for the mode the app
 * is in. Explicit preset colours win; the fallbacks are the base palette's
 * primary and the neutral surfaces, which is close for every built-in base
 * (their backgrounds differ by a hair of chroma).
 */
function presetSwatches(preset: AppearancePreset, dark: boolean): string[] {
  const colors = (dark ? preset.colors.dark : preset.colors.light) ?? {}
  return [
    colors.background ?? (dark ? "oklch(0.145 0 0)" : "oklch(1 0 0)"),
    colors.sidebar ?? (dark ? "oklch(0.205 0 0)" : "oklch(0.985 0 0)"),
    colors.primary ??
      (dark
        ? THEME_COLOR_TITLE[preset.base].dark
        : THEME_COLOR_PREVIEW[preset.base]),
    colors.foreground ?? (dark ? "oklch(0.985 0 0)" : "oklch(0.145 0 0)"),
  ]
}

function readFileText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader()
    reader.onload = () => resolve(String(reader.result ?? ""))
    reader.onerror = () => reject(reader.error ?? new Error("read failed"))
    reader.readAsText(file)
  })
}

function syncAppearanceMode(mode: string) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window))
    return
  import("@/lib/tauri").then((t) =>
    t.updateAppearanceMode(mode).catch(() => {})
  )
}

export function AppearancePresetsSection() {
  const t = useTranslations("AppearanceSettings")
  const { theme, setTheme } = useTheme()
  const {
    themeColor,
    customTheme,
    customThemeEnabled,
    uiFont,
    monoFont,
    codeTheme,
    setCodeTheme,
    appliedPreset,
    applyPreset,
    setSharedThemeToken,
  } = useAppearancePresets()
  const { isDarkMode, customStyleSuppressed } = useCustomStyle()

  const fileInputRef = useRef<HTMLInputElement>(null)
  const [importState, setImportState] = useState<ImportState>(null)

  const current = useMemo<AppearanceCurrent>(
    () => ({
      themeColor,
      customTheme,
      uiFontId: uiFont.id,
      monoFontId: monoFont.id,
      codeTheme,
    }),
    [themeColor, customTheme, uiFont.id, monoFont.id, codeTheme]
  )

  // Which card is lit is a comparison, not a stored flag: the state is what
  // renders, and a preset is "current" exactly when the state equals it.
  const matchedBundled = useMemo(
    () => findMatchingPreset(BUNDLED_PRESETS, current),
    [current]
  )

  // Bundled presets are named through i18n; an imported one carries its own
  // name in the file.
  const bundledName = useCallback(
    (preset: AppearancePreset) => {
      const id = preset.id
      return isBundledPresetId(id)
        ? t(`presets.bundled.${id}.name`)
        : preset.name
    },
    [t]
  )
  const bundledDescription = useCallback(
    (preset: AppearancePreset) => {
      const id = preset.id
      return isBundledPresetId(id)
        ? t(`presets.bundled.${id}.description`)
        : preset.description
    },
    [t]
  )

  const summary = useMemo(() => {
    if (matchedBundled) {
      return t("presets.current", { name: bundledName(matchedBundled) })
    }
    if (appliedPreset) {
      const name = bundledName(appliedPreset)
      return presetMatches(appliedPreset, current)
        ? t("presets.current", { name })
        : t("presets.currentModified", { name })
    }
    return t("presets.currentCustom")
  }, [matchedBundled, appliedPreset, current, bundledName, t])

  // ─── Apply ───

  const onApply = useCallback(
    (preset: AppearancePreset) => {
      applyPreset(preset)
      if (preset.mode) {
        setTheme(preset.mode)
        syncAppearanceMode(preset.mode)
      }
      toast.success(t("presets.toasts.applied", { name: bundledName(preset) }))
    },
    [applyPreset, setTheme, bundledName, t]
  )

  // ─── Export ───

  const onExport = useCallback(async () => {
    const meta = matchedBundled ?? {
      id: "custom",
      name: t("presets.customName"),
    }
    const { preset, warnings } = snapshotToPreset(
      {
        ...current,
        mode: isPresetMode(theme) ? theme : null,
      },
      meta
    )
    try {
      const result = await saveTextFile({
        content: serializeAppearancePreset(preset),
        suggestedName: presetFileName(preset),
        mimeType: "application/json",
        filterName: "codeg preset",
        ext: "json",
      })
      if (result === "cancelled") return
      toast.success(t("presets.toasts.exported"), {
        description:
          warnings.length > 0
            ? t("presets.toasts.partialExport", {
                items: warnings.map((w) => w.path).join(", "),
              })
            : undefined,
      })
    } catch {
      toast.error(t("presets.toasts.exportFailed"))
    }
  }, [matchedBundled, current, theme, t])

  // ─── Import ───

  const onChooseFile = useCallback(
    async (file: File) => {
      if (file.size > MAX_PRESET_BYTES) {
        setImportState({
          kind: "errors",
          issues: [{ path: "", code: "too-large" }],
        })
        return
      }
      let text: string
      try {
        text = await readFileText(file)
      } catch {
        toast.error(t("presets.toasts.readFailed"))
        return
      }
      const result = parseAppearancePreset(text)
      setImportState(
        result.ok
          ? {
              kind: "preview",
              preset: result.preset,
              warnings: result.warnings,
            }
          : { kind: "errors", issues: result.issues }
      )
    },
    [t]
  )

  const onConfirmImport = useCallback(() => {
    if (importState?.kind !== "preview") return
    onApply(importState.preset)
    setImportState(null)
  }, [importState, onApply])

  // ─── Preset-level knobs ───

  const densityValue = useMemo(() => {
    const multiplier = spacingMultiplierFromToken(customTheme.light.spacing)
    if (multiplier === null) {
      return customTheme.light.spacing ? CUSTOM_OPTION : "1"
    }
    const option = DENSITY_OPTIONS.find((o) => o.value === multiplier)
    return option ? String(option.value) : CUSTOM_OPTION
  }, [customTheme.light.spacing])

  const chatTextValue = useMemo(() => {
    const raw = customTheme.light["chat-font-size"]
    if (!raw) return String(DEFAULT_CHAT_FONT_SIZE_REM)
    const parsed = parseCssLength(raw)
    if (!parsed || parsed.unit !== "rem") return CUSTOM_OPTION
    const option = CHAT_TEXT_OPTIONS.find((o) => o.value === parsed.value)
    return option ? String(option.value) : CUSTOM_OPTION
  }, [customTheme.light])

  const onDensityChange = useCallback(
    (value: string) => {
      if (value === CUSTOM_OPTION) return
      const multiplier = Number(value)
      setSharedThemeToken(
        "spacing",
        multiplier === 1 ? null : spacingToken(multiplier)
      )
    },
    [setSharedThemeToken]
  )

  const onChatTextChange = useCallback(
    (value: string) => {
      if (value === CUSTOM_OPTION) return
      const rem = Number(value)
      setSharedThemeToken(
        "chat-font-size",
        rem === DEFAULT_CHAT_FONT_SIZE_REM ? null : `${rem}rem`
      )
    },
    [setSharedThemeToken]
  )

  const knobsDisabled = !customThemeEnabled || customStyleSuppressed
  const fieldLabel = "text-xs font-medium text-muted-foreground"

  const previewPreset =
    importState?.kind === "preview" ? importState.preset : null
  const modeLabel = (mode: PresetMode) => t(`presets.modes.${mode}`)
  const fontLabel = (id: string | undefined) =>
    id ? (FONT_BY_ID[id]?.label ?? id) : t("presets.preview.unchanged")

  return (
    <section className="rounded-xl border bg-card p-4 space-y-4">
      <div className="flex items-center gap-2">
        <SwatchBook className="h-4 w-4 text-muted-foreground" />
        <h2 className="text-sm font-semibold">{t("presets.sectionTitle")}</h2>
      </div>

      <p className="text-xs text-muted-foreground leading-5">
        {t("presets.sectionDescription")}
      </p>

      {/* ===== Gallery ===== */}
      <div
        className="grid grid-cols-1 gap-2 sm:grid-cols-2 lg:grid-cols-3"
        role="group"
        aria-label={t("presets.sectionTitle")}
      >
        {BUNDLED_PRESETS.map((preset) => {
          const active = matchedBundled?.id === preset.id
          const name = bundledName(preset)
          return (
            <button
              key={preset.id}
              type="button"
              onClick={() => onApply(preset)}
              aria-pressed={active}
              aria-label={t("presets.apply", { name })}
              className={cn(
                "flex flex-col gap-2 rounded-md border p-3 text-left transition-colors",
                "hover:bg-accent hover:text-accent-foreground",
                active && "border-primary ring-2 ring-primary/30"
              )}
            >
              <span className="flex items-center gap-1.5" aria-hidden>
                {presetSwatches(preset, isDarkMode).map((color, index) => (
                  <span
                    key={index}
                    className="size-4 rounded-full border"
                    style={{ backgroundColor: color }}
                  />
                ))}
              </span>
              <span className="text-xs font-medium">{name}</span>
              <span className="line-clamp-2 text-2xs text-muted-foreground leading-4">
                {bundledDescription(preset)}
              </span>
            </button>
          )
        })}
      </div>

      <p className="text-2xs text-muted-foreground">{summary}</p>
      {knobsDisabled && (
        <p className="text-2xs text-muted-foreground leading-4">
          {t("presets.disabledHint")}
        </p>
      )}

      {/* ===== Code colours ===== */}
      <div className="space-y-2">
        <label className={fieldLabel}>{t("presets.codeTheme")}</label>
        <div className="flex flex-wrap items-center gap-2">
          <Select
            value={codeTheme.light}
            onValueChange={(value) => {
              if (isCodeThemeId(value)) {
                setCodeTheme({ ...codeTheme, light: value })
              }
            }}
          >
            <SelectTrigger
              className="w-full sm:w-56"
              aria-label={t("presets.codeThemeLight")}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="start">
              <SelectGroup>
                <SelectLabel>{t("presets.codeThemeGroupLight")}</SelectLabel>
                {LIGHT_CODE_THEMES.map((theme) => (
                  <SelectItem key={theme.id} value={theme.id}>
                    {codeThemeLabel(theme.id)}
                  </SelectItem>
                ))}
              </SelectGroup>
              <SelectGroup>
                <SelectLabel>{t("presets.codeThemeGroupDark")}</SelectLabel>
                {DARK_CODE_THEMES.map((theme) => (
                  <SelectItem key={theme.id} value={theme.id}>
                    {codeThemeLabel(theme.id)}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
          <Select
            value={codeTheme.dark}
            onValueChange={(value) => {
              if (isCodeThemeId(value)) {
                setCodeTheme({ ...codeTheme, dark: value })
              }
            }}
          >
            <SelectTrigger
              className="w-full sm:w-56"
              aria-label={t("presets.codeThemeDark")}
            >
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="start">
              <SelectGroup>
                <SelectLabel>{t("presets.codeThemeGroupDark")}</SelectLabel>
                {DARK_CODE_THEMES.map((theme) => (
                  <SelectItem key={theme.id} value={theme.id}>
                    {codeThemeLabel(theme.id)}
                  </SelectItem>
                ))}
              </SelectGroup>
              <SelectGroup>
                <SelectLabel>{t("presets.codeThemeGroupLight")}</SelectLabel>
                {LIGHT_CODE_THEMES.map((theme) => (
                  <SelectItem key={theme.id} value={theme.id}>
                    {codeThemeLabel(theme.id)}
                  </SelectItem>
                ))}
              </SelectGroup>
            </SelectContent>
          </Select>
        </div>
        <p className="text-2xs text-muted-foreground leading-4">
          {t("presets.codeThemeHint")}
        </p>
      </div>

      {/* ===== Density + message text ===== */}
      <div className="flex flex-wrap items-start gap-4">
        <div className="space-y-2">
          <label className={fieldLabel}>{t("presets.density")}</label>
          <Select
            value={densityValue}
            onValueChange={onDensityChange}
            disabled={knobsDisabled}
          >
            <SelectTrigger className="w-40" aria-label={t("presets.density")}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="start">
              {DENSITY_OPTIONS.map((option) => (
                <SelectItem key={option.key} value={String(option.value)}>
                  {t(`presets.densityOptions.${option.key}`)}
                </SelectItem>
              ))}
              {densityValue === CUSTOM_OPTION && (
                <SelectItem value={CUSTOM_OPTION} disabled>
                  {t("presets.densityOptions.custom")}
                </SelectItem>
              )}
            </SelectContent>
          </Select>
          <p className="text-2xs text-muted-foreground leading-4">
            {t("presets.densityHint")}
          </p>
        </div>
        <div className="space-y-2">
          <label className={fieldLabel}>{t("presets.chatText")}</label>
          <Select
            value={chatTextValue}
            onValueChange={onChatTextChange}
            disabled={knobsDisabled}
          >
            <SelectTrigger className="w-40" aria-label={t("presets.chatText")}>
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="start">
              {CHAT_TEXT_OPTIONS.map((option) => (
                <SelectItem key={option.key} value={String(option.value)}>
                  {t(`presets.chatTextOptions.${option.key}`)}
                </SelectItem>
              ))}
              {chatTextValue === CUSTOM_OPTION && (
                <SelectItem value={CUSTOM_OPTION} disabled>
                  {t("presets.chatTextOptions.custom")}
                </SelectItem>
              )}
            </SelectContent>
          </Select>
          <p className="text-2xs text-muted-foreground leading-4">
            {t("presets.chatTextHint")}
          </p>
        </div>
      </div>

      {/* ===== Import / export ===== */}
      <div className="flex flex-wrap items-center gap-2 border-t pt-4">
        <input
          ref={fileInputRef}
          type="file"
          accept={PRESET_ACCEPT}
          className="hidden"
          data-testid="preset-file-input"
          onChange={(e) => {
            const file = e.target.files?.[0]
            if (file) void onChooseFile(file)
            e.target.value = ""
          }}
        />
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => fileInputRef.current?.click()}
        >
          <Upload className="size-3.5" />
          {t("presets.import")}
        </Button>
        <Button
          type="button"
          variant="outline"
          size="sm"
          onClick={() => void onExport()}
        >
          <Download className="size-3.5" />
          {t("presets.export")}
        </Button>
      </div>
      <p className="text-2xs text-muted-foreground leading-4">
        {t("presets.sharingNote")}
      </p>

      <Dialog
        open={importState !== null}
        onOpenChange={(open) => {
          if (!open) setImportState(null)
        }}
      >
        <DialogContent className="sm:max-w-lg">
          {previewPreset ? (
            <>
              <DialogHeader>
                <DialogTitle>{t("presets.importTitle")}</DialogTitle>
                <DialogDescription>
                  {t("presets.importPreview")}
                </DialogDescription>
              </DialogHeader>
              <div className="space-y-2 text-xs">
                <p className="text-sm font-medium">{previewPreset.name}</p>
                {previewPreset.description && (
                  <p className="text-muted-foreground leading-5">
                    {previewPreset.description}
                  </p>
                )}
                <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-2xs">
                  <dt className="text-muted-foreground">
                    {t("presets.preview.base")}
                  </dt>
                  <dd>{t(`themeColor.options.${previewPreset.base}`)}</dd>
                  <dt className="text-muted-foreground">
                    {t("presets.preview.mode")}
                  </dt>
                  <dd>
                    {previewPreset.mode
                      ? modeLabel(previewPreset.mode)
                      : t("presets.preview.unchanged")}
                  </dd>
                  <dt className="text-muted-foreground">
                    {t("presets.preview.overrides")}
                  </dt>
                  <dd>
                    {t("presets.preview.overridesValue", {
                      light: Object.keys(previewPreset.colors.light ?? {})
                        .length,
                      dark: Object.keys(previewPreset.colors.dark ?? {}).length,
                    })}
                  </dd>
                  <dt className="text-muted-foreground">
                    {t("presets.preview.fonts")}
                  </dt>
                  <dd>
                    {fontLabel(previewPreset.typography?.ui)} /{" "}
                    {fontLabel(previewPreset.typography?.mono)}
                  </dd>
                  <dt className="text-muted-foreground">
                    {t("presets.preview.code")}
                  </dt>
                  <dd>
                    {previewPreset.code
                      ? `${codeThemeLabel(previewPreset.code.light)} / ${codeThemeLabel(previewPreset.code.dark)}`
                      : t("presets.preview.unchanged")}
                  </dd>
                  <dt className="text-muted-foreground">
                    {t("presets.preview.density")}
                  </dt>
                  <dd>
                    {previewPreset.density?.spacing !== undefined
                      ? `${Math.round(previewPreset.density.spacing * 100)}%`
                      : t("presets.preview.unchanged")}
                  </dd>
                  <dt className="text-muted-foreground">
                    {t("presets.preview.chatText")}
                  </dt>
                  <dd>
                    {previewPreset.typography?.chatFontSize !== undefined
                      ? `${previewPreset.typography.chatFontSize}rem`
                      : t("presets.preview.unchanged")}
                  </dd>
                </dl>
              </div>
              <DialogFooter>
                <Button variant="ghost" onClick={() => setImportState(null)}>
                  {t("presets.cancel")}
                </Button>
                <Button onClick={onConfirmImport}>
                  {t("presets.importApply")}
                </Button>
              </DialogFooter>
            </>
          ) : (
            <>
              <DialogHeader>
                <DialogTitle>{t("presets.importErrorsTitle")}</DialogTitle>
                <DialogDescription>
                  {t("presets.importErrorsDescription")}
                </DialogDescription>
              </DialogHeader>
              <ul
                className="max-h-64 space-y-1 overflow-auto font-mono text-2xs"
                aria-label={t("presets.importErrorsTitle")}
              >
                {(importState?.kind === "errors" ? importState.issues : []).map(
                  (issue, index) => (
                    <li key={`${issue.path}-${index}`}>
                      {issue.path ? `${issue.path}: ` : ""}
                      {t(`presets.errors.${issue.code}`, {
                        limit: Math.round(MAX_PRESET_BYTES / 1024),
                      })}
                      {issue.detail ? ` (${issue.detail})` : ""}
                    </li>
                  )
                )}
              </ul>
              <DialogFooter>
                <Button variant="ghost" onClick={() => setImportState(null)}>
                  {t("presets.cancel")}
                </Button>
              </DialogFooter>
            </>
          )}
        </DialogContent>
      </Dialog>
    </section>
  )
}
