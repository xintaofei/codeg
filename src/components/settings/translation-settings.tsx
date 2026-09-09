"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import {
  ChevronDown,
  HelpCircle,
  Languages,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
  SlidersHorizontal,
  Trash2,
} from "lucide-react"
import { useLocale, useTranslations } from "next-intl"
import { toast } from "sonner"

import { SettingsSection } from "@/components/shared/settings-section"
import { SettingCard, SettingRow } from "@/components/shared/setting-card"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import { Switch } from "@/components/ui/switch"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import {
  Command,
  CommandGroup,
  CommandItem,
  CommandList,
} from "@/components/ui/command"
import { Checkbox } from "@/components/ui/checkbox"
import { cn } from "@/lib/utils"
import {
  getTranslationSettings,
  getTranslationStats,
  listTranslationModels,
  testTranslationSettings,
  updateTranslationSettings,
} from "@/lib/api"
import { APP_LOCALES, toIntlLocale } from "@/lib/i18n"
import { toErrorMessage } from "@/lib/app-error"
import type {
  AppLocale,
  TranslationApiFormat,
  TranslationProvider,
  TranslationSettings,
} from "@/lib/types"
import { primeTranslationSettings } from "@/hooks/use-translated-text"

const TARGET_LANG_OPTIONS = [
  { value: "__interface__", localeKey: null },
  ...APP_LOCALES.map((locale) => ({
    value: toIntlLocale(locale),
    localeKey: locale,
  })),
]

/** Dialect pins for the endpoint; brand names stay untranslated. */
const API_FORMAT_OPTIONS: { value: TranslationApiFormat; label: string }[] = [
  { value: "auto", label: "Auto" },
  { value: "openai", label: "OpenAI" },
  { value: "anthropic", label: "Claude" },
  { value: "gemini", label: "Gemini" },
  { value: "ollama", label: "Ollama" },
]

/** Soft client-side bounds for the numeric fields; the backend clamps too. */
const RPM_CAP_BOUNDS = { min: 2, max: 600 }
const BATCH_CHARS_BOUNDS = { min: 500, max: 20_000 }
/** Per-lane concurrency ceilings (priority / background); backend clamps too. */
const LANE_BOUNDS = { min: 1, max: 16 }
/** Failure-strategy ceilings: consecutive-failure trigger and parking window. */
const FAILURE_THRESHOLD_BOUNDS = { min: 1, max: 20 }
const COOLDOWN_SECONDS_BOUNDS = { min: 5, max: 3600 }

/**
 * The lanes the scope multi-select offers, in the order their old switches
 * stacked. `key`/`labelKey` drive the checkbox row; `label` (read off the
 * current translator for the trigger summary) is resolved at render time
 * inside the component, where the translator lives.
 */
const SCOPE_OPTIONS = [
  { key: "translateBody", value: "body", labelKey: "translateBodyLabel" },
  {
    key: "translateThinking",
    value: "thinking",
    labelKey: "translateThinkingLabel",
  },
  {
    key: "selectionTranslate",
    value: "selection",
    labelKey: "selectionTranslateLabel",
  },
] as const

/** `api.example.com/v1` → `api.example.com`: host only, scheme and path off.
 *  Used by the key-waiver check below. */
function hostOf(baseUrl: string): string {
  const afterScheme = baseUrl.trim().split("://").pop() ?? ""
  return afterScheme.split(/[/?#]/)[0] ?? ""
}

/**
 * A new endpoint row's identity. Generated client-side so the "test
 * connection" and "fetch models" buttons can aim at the exact row being
 * edited even before the first save; the backend keeps the id on save.
 */
function newProviderId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID()
  }
  return `p-${Date.now()}-${Math.random().toString(36).slice(2, 10)}`
}

function emptyProvider(): TranslationProvider {
  return {
    id: newProviderId(),
    name: null,
    baseUrl: "",
    apiKey: "",
    model: "",
    apiFormat: "auto",
    enabled: true,
    rpmCap: null,
  }
}

/**
 * What the switches on this page share: none of them reaches the backend (or
 * the renderer's snapshot) until 保存 runs, so the save button says so once
 * the form drifts from the persisted state.
 */
function formFingerprint(
  settings: TranslationSettings,
  targetLang: string | null
): string {
  return JSON.stringify([
    settings.enabled,
    settings.providers,
    settings.batchMaxChars,
    settings.carryContext,
    settings.translateBody,
    settings.translateThinking,
    settings.priorityMaxConcurrent,
    settings.backgroundMaxConcurrent,
    settings.selectionTranslate,
    settings.selectionTargetLang,
    settings.toggleAlwaysVisible,
    targetLang,
  ])
}

/** Placeholder hints the user toward a model each dialect actually serves. */
const MODEL_PLACEHOLDER_BY_FORMAT: Record<TranslationApiFormat, string> = {
  auto: "gpt-4o-mini",
  openai: "gpt-4o-mini",
  anthropic: "claude-sonnet-4-5",
  gemini: "gemini-2.5-flash",
  ollama: "qwen2.5:14b",
}

/**
 * Whether the backend would waive the API key for this draft: it does for
 * Ollama, which serves locally with none. Under `auto` the dialect is read
 * off the host, so the two signals the backend checks — an `ollama` host or
 * its default port — gate the button here too. The backend stays
 * authoritative: a permissive guess can only end in a toast, while a strict
 * one would strand the button with no feedback at all.
 */
function keyIsWaived(
  baseUrl: string,
  apiFormat: TranslationApiFormat
): boolean {
  if (apiFormat === "ollama") return true
  if (apiFormat !== "auto") return false
  const hostAndPort = hostOf(baseUrl).toLowerCase()
  return hostAndPort.includes("ollama") || hostAndPort.endsWith(":11434")
}

/** Whether asking the endpoint for its models could mean anything yet. */
function canProbeModels(provider: TranslationProvider): boolean {
  if (!provider.baseUrl.trim()) return false
  return (
    keyIsWaived(provider.baseUrl, provider.apiFormat) ||
    provider.apiKey.trim().length > 0
  )
}

/**
 * `Language` is keyed by language name, not by locale code, so an `AppLocale`
 * cannot be handed to the translator directly (`Language.zh_cn` does not
 * exist). Same mapping the system settings language picker uses.
 */
const LANGUAGE_LABEL_KEYS = {
  en: "english",
  zh_cn: "simplifiedChinese",
  zh_tw: "traditionalChinese",
  ja: "japanese",
  ko: "korean",
  es: "spanish",
  de: "german",
  fr: "french",
  pt: "portuguese",
  ar: "arabic",
} as const satisfies Record<AppLocale, string>

/**
 * Backend validation messages this page can surface, mapped to their i18n
 * keys. The backend speaks English constants; the map is exact-match, so any
 * message it does not know falls back to the original text rather than a
 * guess — a toast in English beats a toast in the wrong language.
 */
const BACKEND_ERROR_KEYS: Record<string, string> = {
  "Translation needs at least one enabled provider with a base URL, an API key, and a model":
    "errNeedsEnabledProvider",
  "Unknown translation API format": "errUnknownApiFormat",
  "Translation API key is too long": "errApiKeyTooLong",
  "Translation model name is too long": "errModelTooLong",
  "Translation provider name is too long": "errProviderNameTooLong",
  "Translation base URL is too long": "errBaseUrlTooLong",
  "Translation base URL scheme must be http:// or https://": "errBaseUrlScheme",
  "Translation base URL is not a valid URL": "errBaseUrlInvalid",
  "Translation base URL must include a host": "errBaseUrlNoHost",
  "Translation target language is too long": "errTargetLangTooLong",
  "This endpoint does not expose a model list — enter the model name manually":
    "errNoModelList",
  "Fill in the provider's base URL and key before fetching models":
    "errFillProviderForModels",
  "The translation endpoint did not respond within 150 seconds":
    "errTestTimeout",
}

export function TranslationSettings() {
  const t = useTranslations("TranslationSettings")
  const tLanguage = useTranslations("Language")
  const locale = useLocale()

  // Show a backend failure in the interface's language: known validation
  // messages translate through BACKEND_ERROR_KEYS, everything else —
  // endpoint bodies, transport errors — passes through untouched.
  const localizeBackendError = useCallback(
    (err: unknown): string => {
      const raw = toErrorMessage(err)
      const key = BACKEND_ERROR_KEYS[raw]
      // The map values are compile-time constants; the lookup key is runtime
      // data, so the translator's literal-key type needs this one escape.
      return key ? (t as (k: string) => string)(key) : raw
    },
    [t]
  )

  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [testing, setTesting] = useState(false)
  const [settings, setSettings] = useState<TranslationSettings>({
    enabled: false,
    providers: [],
    baseUrl: "",
    apiKey: "",
    model: "",
    targetLang: null,
    translateBody: true,
    translateThinking: false,
    apiFormat: "auto",
    selectionTranslate: true,
    selectionTargetLang: null,
    toggleAlwaysVisible: false,
    priorityMaxConcurrent: null,
    backgroundMaxConcurrent: null,
    batchMaxChars: null,
    failureThreshold: null,
    cooldownSeconds: null,
    carryContext: true,
  })
  /**
   * Which endpoint row the editor card is open for, or `null` when the card
   * is collapsed — the list alone reads much cleaner, and the endpoint
   * fields only matter while adding or editing one row.
   */
  const [editingIndex, setEditingIndex] = useState<number | null>(null)
  const [targetLang, setTargetLang] = useState<string>("__interface__")
  /** The in-memory counters the backend reports; informational only. */
  const [stats, setStats] = useState<{
    requests: number
    ok: number
    rejected: number
    failures: number
    avgLatencyMs: number
  } | null>(null)
  const [modelProbe, setModelProbe] = useState<
    | {
        baseUrl: string
        apiKey: string
        apiFormat: TranslationApiFormat
        kind: "ok"
        models: string[]
      }
    | {
        baseUrl: string
        apiKey: string
        apiFormat: TranslationApiFormat
        kind: "empty"
      }
    | null
  >(null)
  const [fetchingModels, setFetchingModels] = useState(false)
  /** The last state known to be persisted; drives the unsaved-changes hint. */
  const [savedSnapshot, setSavedSnapshot] = useState<string | null>(null)
  /** The latest persisted settings, for the per-provider key refill on save. */
  const storedRef = useRef<TranslationSettings | null>(null)
  /** The endpoint editor card, for scrolling it into view when it opens. */
  const editorCardRef = useRef<HTMLDivElement | null>(null)
  /**
   * Per-provider connection-test verdicts from the last 测试连接 run. They
   * drive the 状态 column — a row that was never tested has no state to
   * show, and "the test just failed" must survive re-renders.
   */
  const [providerTestState, setProviderTestState] = useState<
    Record<string, { state: "testing" | "ok" | "failed"; message?: string }>
  >({})
  /** Whether the translation-scope multi-select popover is expanded. */
  const [scopeOpen, setScopeOpen] = useState(false)

  const loadStats = useCallback(async () => {
    try {
      setStats(await getTranslationStats())
    } catch {
      // The counters are informational; a failure must not block the page.
    }
  }, [])

  useEffect(() => {
    let active = true
    getTranslationSettings()
      .then((stored) => {
        if (!active) return
        storedRef.current = stored
        // A fresh install (or a disabled row with no list) gets one empty
        // draft to fill — and the editor opens on it, since there is nothing
        // else on the card to look at. Otherwise the card stays collapsed
        // behind the list until the user edits a row.
        const seeded: TranslationSettings =
          stored.providers.length > 0
            ? stored
            : { ...stored, providers: [emptyProvider()] }
        setSettings(seeded)
        setEditingIndex(stored.providers.length > 0 ? null : 0)
        setTargetLang(seeded.targetLang ?? "__interface__")
        setSavedSnapshot(formFingerprint(seeded, seeded.targetLang))
        primeTranslationSettings(stored)
      })
      .catch(() => {
        if (active) toast.error(t("loadFailed"))
      })
      .finally(() => {
        if (active) setLoading(false)
      })
    loadStats()
    return () => {
      active = false
    }
  }, [t, loadStats])

  const provider =
    editingIndex !== null
      ? (settings.providers[editingIndex] ?? emptyProvider())
      : undefined

  const updateProvider = useCallback(
    (patch: Partial<TranslationProvider>) => {
      if (editingIndex === null) return
      const index = editingIndex
      setSettings((prev) => {
        const providers = [...prev.providers]
        providers[index] = {
          ...(providers[index] ?? emptyProvider()),
          ...patch,
        }
        return { ...prev, providers }
      })
    },
    [editingIndex]
  )

  const addProvider = useCallback(() => {
    setSettings((prev) => {
      const providers = [...prev.providers, emptyProvider()]
      return { ...prev, providers }
    })
    setEditingIndex(settings.providers.length)
    // The editor card lives below the list, which just grew a row — without
    // this the card opens off-screen and clicking 添加供应商 reads as "nothing
    // happened". One frame later, once the card exists to be scrolled to.
    requestAnimationFrame(() => {
      editorCardRef.current?.scrollIntoView({
        behavior: "smooth",
        block: "nearest",
      })
    })
  }, [settings.providers.length])

  const removeProvider = useCallback((index: number) => {
    setSettings((prev) => {
      // The last row is never removed: an empty list would fall back to the
      // legacy flat fields on save, which this form no longer edits.
      if (prev.providers.length <= 1) return prev
      const providers = prev.providers.filter((_, i) => i !== index)
      return { ...prev, providers }
    })
    setEditingIndex((prev) =>
      prev === null
        ? null
        : prev === index
          ? null
          : prev > index
            ? prev - 1
            : prev
    )
  }, [])

  const closeEditor = useCallback(() => setEditingIndex(null), [])

  const handleTestConnection = useCallback(async () => {
    setTesting(true)
    // Every configured endpoint gets its own English test sentence, in
    // parallel; the 状态 cell turns 正常 or 不可用 per row as the results land.
    const rows = settings.providers
    setProviderTestState(
      Object.fromEntries(
        rows.map((row) => [row.id, { state: "testing" as const }])
      )
    )
    const payload: TranslationSettings = {
      ...settings,
      targetLang: targetLang === "__interface__" ? null : targetLang,
    }
    const results = await Promise.allSettled(
      rows.map((row) =>
        testTranslationSettings(payload, locale, row.id || null)
      )
    )
    const next: Record<string, { state: "ok" | "failed"; message?: string }> =
      {}
    let okCount = 0
    rows.forEach((row, index) => {
      const outcome = results[index]
      if (outcome.status === "fulfilled") {
        next[row.id] = { state: "ok" }
        okCount += 1
      } else {
        next[row.id] = {
          state: "failed",
          message: localizeBackendError(outcome.reason),
        }
      }
    })
    setProviderTestState(next)
    if (okCount === rows.length) {
      toast.success(t("testSummaryAll", { count: rows.length }))
    } else {
      toast.error(t("testSummaryPartial", { ok: okCount, total: rows.length }))
    }
    setTesting(false)
  }, [settings, targetLang, locale, t, localizeBackendError])

  const handleSave = useCallback(async () => {
    setSaving(true)
    try {
      // The form never holds the real per-provider keys (masked reads); echo
      // the stored key back for any row still showing the mask so the backend
      // can merge by id.
      const stored = storedRef.current
      const payload: TranslationSettings = {
        ...settings,
        targetLang: targetLang === "__interface__" ? null : targetLang,
        providers: settings.providers.map((row) =>
          row.apiKey === "••••••••"
            ? {
                ...row,
                apiKey:
                  stored?.providers.find((p) => p.id === row.id)?.apiKey ??
                  row.apiKey,
              }
            : row
        ),
      }
      const saved = await updateTranslationSettings(payload)
      storedRef.current = saved
      primeTranslationSettings(saved)
      toast.success(t("saved"))
      const refreshed = await getTranslationSettings()
      setSettings(refreshed)
      setEditingIndex((prev) =>
        prev === null || refreshed.providers.length === 0
          ? null
          : Math.min(prev, refreshed.providers.length - 1)
      )
      setSavedSnapshot(
        formFingerprint(
          refreshed,
          targetLang === "__interface__" ? null : targetLang
        )
      )
    } catch (err) {
      toast.error(localizeBackendError(err))
    } finally {
      setSaving(false)
    }
  }, [settings, targetLang, t, localizeBackendError])

  const handleFetchModels = useCallback(async () => {
    // The backend classifies 401/404/timeout distinctly; a defensive return
    // here only covers the case no request could meaningfully describe. The
    // button lives inside the editor card, so a provider is always in scope.
    if (!provider || !canProbeModels(provider)) return

    setFetchingModels(true)
    try {
      const models = await listTranslationModels(settings, provider.id || null)
      setModelProbe(
        models.length > 0
          ? {
              baseUrl: provider.baseUrl,
              apiKey: provider.apiKey,
              apiFormat: provider.apiFormat,
              kind: "ok",
              models,
            }
          : {
              baseUrl: provider.baseUrl,
              apiKey: provider.apiKey,
              apiFormat: provider.apiFormat,
              kind: "empty",
            }
      )
    } catch (err) {
      toast.error(t("fetchModelsFailed", { error: localizeBackendError(err) }))
    } finally {
      setFetchingModels(false)
    }
  }, [provider, settings, t, localizeBackendError])

  // A model list only speaks for the credentials it was fetched against. Once
  // the endpoint, key, or dialect moves it is dropped rather than left as
  // suggestions for a request that would no longer be the one issued
  // (same mechanism the kimi-code panel uses). The raw values match the
  // request, not their trimmed forms — editing either is the user's signal.
  const activeProbe =
    modelProbe &&
    provider &&
    modelProbe.baseUrl === provider.baseUrl &&
    modelProbe.apiKey === provider.apiKey &&
    modelProbe.apiFormat === provider.apiFormat
      ? modelProbe
      : null

  const fetchedModels = activeProbe?.kind === "ok" ? activeProbe.models : []
  const showEmptyHint = activeProbe?.kind === "empty"

  const hasUnsavedChanges =
    savedSnapshot !== null &&
    savedSnapshot !==
      formFingerprint(
        settings,
        targetLang === "__interface__" ? null : targetLang
      )

  if (loading) {
    return (
      <div className="flex h-full items-center justify-center">
        <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
      </div>
    )
  }

  const languageLabel = (localeKey: AppLocale | null) =>
    localeKey === null
      ? t("targetLangFollowInterface")
      : tLanguage(LANGUAGE_LABEL_KEYS[localeKey])

  /** The checked lanes, for the trigger summary. Labels resolve through the
   *  translator here — the module-level option list only carries keys. */
  const selectedScopes = SCOPE_OPTIONS.filter(
    (scope) => settings[scope.key]
  ).map((scope) => ({ key: scope.key, label: t(scope.labelKey) }))

  const stateForProvider = (id: string | undefined) =>
    id ? providerTestState[id] : undefined

  const bindNumber = (
    value: number | null,
    bounds: { min: number; max: number },
    onChange: (value: number | null) => void,
    /** Shown when empty — the actual default, not just the word "default". */
    emptyHint: string
  ) => ({
    type: "number" as const,
    inputMode: "numeric" as const,
    min: bounds.min,
    max: bounds.max,
    // Empty means "use the default" (`null` end to end). Typing stays
    // unclamped so intermediate keystrokes (e.g. "1" on the way to "1500")
    // aren't rewritten; the backend clamps the authoritative value on save.
    value: value?.toString() ?? "",
    placeholder: emptyHint,
    onChange: (event: React.ChangeEvent<HTMLInputElement>) => {
      const raw = event.target.value.trim()
      if (raw === "") {
        onChange(null)
        return
      }
      const parsed = Number(raw)
      onChange(Number.isFinite(parsed) ? parsed : null)
    },
  })

  return (
    <ScrollArea className="h-full">
      <section className="mx-auto max-w-2xl space-y-3 px-3 pt-3 md:px-4 md:pt-4">
        <SettingsSection
          icon={Languages}
          title={t("sectionTitle")}
          description={t("sectionDescription")}
        >
          <SettingCard className="divide-y">
            <SettingRow
              title={t("enabledLabel")}
              description={t("enabledDescription")}
              htmlFor="translation-enabled"
              control={
                <Switch
                  id="translation-enabled"
                  checked={settings.enabled}
                  onCheckedChange={(checked) =>
                    setSettings((prev) => ({ ...prev, enabled: checked }))
                  }
                />
              }
            />
            <SettingRow
              title={t("targetLangLabel")}
              htmlFor="translation-target-lang"
            >
              <Select value={targetLang} onValueChange={setTargetLang}>
                {/* Fluid up to the old fixed width: the row's control column
                    is `shrink-0`, so a hard `w-64` pushed the title off a
                    narrow settings pane. */}
                <SelectTrigger
                  id="translation-target-lang"
                  className="h-8 w-full max-w-64 text-xs"
                >
                  <SelectValue />
                </SelectTrigger>
                <SelectContent>
                  {TARGET_LANG_OPTIONS.map((option) => (
                    <SelectItem key={option.value} value={option.value}>
                      {languageLabel(option.localeKey)}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </SettingRow>
            {/* The three scope choices read as one decision, so they share a
                row: the multi-select names every lane that is on, and the
                selection target only matters while 划词 is one of them. */}
            <SettingRow
              title={t("scopeTitle")}
              description={t("scopeDescription")}
              htmlFor="translation-scope-trigger"
              control={
                <Popover open={scopeOpen} onOpenChange={setScopeOpen}>
                  <PopoverTrigger asChild>
                    <Button
                      id="translation-scope-trigger"
                      type="button"
                      variant="outline"
                      size="sm"
                      role="combobox"
                      aria-expanded={scopeOpen}
                      aria-label={t("scopeAriaLabel")}
                      className={cn(
                        "w-56 justify-between gap-1 px-3 font-normal",
                        selectedScopes.length === 0 && "text-muted-foreground"
                      )}
                    >
                      <span className="min-w-0 truncate text-start text-xs">
                        {selectedScopes.length > 0
                          ? selectedScopes
                              .map((scope) => scope.label)
                              .join("、")
                          : t("scopeNoneSelected")}
                      </span>
                      <ChevronDown
                        className="size-3.5 shrink-0 text-muted-foreground/60"
                        aria-hidden="true"
                      />
                    </Button>
                  </PopoverTrigger>
                  <PopoverContent align="end" className="w-56 p-1">
                    <Command>
                      <CommandList>
                        <CommandGroup>
                          {SCOPE_OPTIONS.map((scope) => {
                            const checked = settings[scope.key]
                            return (
                              <CommandItem
                                key={scope.key}
                                value={scope.value}
                                onSelect={() =>
                                  setSettings((prev) => ({
                                    ...prev,
                                    [scope.key]: !prev[scope.key],
                                  }))
                                }
                              >
                                <Checkbox
                                  checked={checked}
                                  aria-hidden="true"
                                />
                                <span className="min-w-0 flex-1 text-xs">
                                  {t(scope.labelKey)}
                                </span>
                              </CommandItem>
                            )
                          })}
                        </CommandGroup>
                      </CommandList>
                    </Command>
                  </PopoverContent>
                </Popover>
              }
            />
            {settings.selectionTranslate && (
              <SettingRow
                title={t("selectionTargetLangLabel")}
                htmlFor="translation-selection-lang"
              >
                <Select
                  value={settings.selectionTargetLang ?? "__target__"}
                  onValueChange={(value) =>
                    setSettings((prev) => ({
                      ...prev,
                      selectionTargetLang:
                        value === "__target__" ? null : value,
                    }))
                  }
                >
                  <SelectTrigger
                    id="translation-selection-lang"
                    className="h-8 w-full max-w-64 text-xs"
                  >
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="__target__">
                      {t("selectionTargetLangFollow")}
                    </SelectItem>
                    {TARGET_LANG_OPTIONS.filter(
                      (option) => option.value !== "__interface__"
                    ).map((option) => (
                      <SelectItem key={option.value} value={option.value}>
                        {languageLabel(option.localeKey)}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </SettingRow>
            )}
            <SettingRow
              title={t("toggleAlwaysVisibleLabel")}
              description={t("toggleAlwaysVisibleDescription")}
              htmlFor="translation-toggle-visible"
              control={
                <Switch
                  id="translation-toggle-visible"
                  checked={settings.toggleAlwaysVisible}
                  onCheckedChange={(checked) =>
                    setSettings((prev) => ({
                      ...prev,
                      toggleAlwaysVisible: checked,
                    }))
                  }
                />
              }
            />
            <SettingRow
              title={t("batchMaxCharsLabel")}
              description={t("batchMaxCharsDescription")}
              htmlFor="translation-batch-chars"
            >
              <div className="flex items-center gap-1">
                <FieldHelp
                  title={t("helpBatchTitle")}
                  body={t("helpBatchBody")}
                  label={t("batchMaxCharsLabel")}
                />
                <Input
                  id="translation-batch-chars"
                  className="h-8 w-32 text-xs"
                  {...bindNumber(
                    settings.batchMaxChars,
                    BATCH_CHARS_BOUNDS,
                    (value) =>
                      setSettings((prev) => ({
                        ...prev,
                        batchMaxChars: value,
                      })),
                    t("batchDefaultHint")
                  )}
                />
              </div>
            </SettingRow>
            <SettingRow
              title={t("priorityConcurrentLabel")}
              description={t("priorityConcurrentDescription")}
              htmlFor="translation-priority-concurrent"
            >
              <Input
                id="translation-priority-concurrent"
                className="h-8 w-32 text-xs"
                {...bindNumber(
                  settings.priorityMaxConcurrent,
                  LANE_BOUNDS,
                  (value) =>
                    setSettings((prev) => ({
                      ...prev,
                      priorityMaxConcurrent: value,
                    })),
                  t("priorityConcurrentDefaultHint")
                )}
              />
            </SettingRow>
            <SettingRow
              title={t("backgroundConcurrentLabel")}
              description={t("backgroundConcurrentDescription")}
              htmlFor="translation-background-concurrent"
            >
              <Input
                id="translation-background-concurrent"
                className="h-8 w-32 text-xs"
                {...bindNumber(
                  settings.backgroundMaxConcurrent,
                  LANE_BOUNDS,
                  (value) =>
                    setSettings((prev) => ({
                      ...prev,
                      backgroundMaxConcurrent: value,
                    })),
                  t("backgroundConcurrentDefaultHint")
                )}
              />
            </SettingRow>
            <SettingRow
              title={t("carryContextLabel")}
              description={t("carryContextDescription")}
              htmlFor="translation-carry-context"
              control={
                <Switch
                  id="translation-carry-context"
                  checked={settings.carryContext}
                  onCheckedChange={(checked) =>
                    setSettings((prev) => ({ ...prev, carryContext: checked }))
                  }
                />
              }
            />
          </SettingCard>
        </SettingsSection>

        <SettingsSection title={t("providersTitle")}>
          <SettingCard className="divide-y">
            {/*
              A real table so the column headers and every data row align:
              供应商 | 模型 | 状态 | (行操作). `table-fixed` sizes the columns
              off the header widths — each row used to be its own grid, so
              content-sized tracks drifted out from under their headers.
            */}
            <table className="w-full table-fixed text-xs">
              <thead>
                <tr className="bg-muted/30 text-muted-foreground">
                  <th className="w-[30%] truncate px-3 py-2.5 text-left font-normal">
                    {t("colProvider")}
                  </th>
                  <th className="w-[26%] truncate px-3 py-2.5 text-left font-normal">
                    {t("colModel")}
                  </th>
                  <th className="w-[24%] truncate px-3 py-2.5 text-left font-normal">
                    {t("colState")}
                  </th>
                  {/* The failure strategy lives behind a header popover:
                      pool tuning is rare, and a dedicated dialog would
                      outweigh two numbers. */}
                  <th className="w-[20%] px-3 py-2.5 text-right font-normal">
                    <Popover>
                      <PopoverTrigger asChild>
                        <Button
                          type="button"
                          variant="ghost"
                          size="xs"
                          aria-label={t("failureStrategy")}
                        >
                          <SlidersHorizontal className="size-3.5" />
                          {t("failureStrategy")}
                        </Button>
                      </PopoverTrigger>
                      <PopoverContent align="end" className="w-64 space-y-2">
                        <div className="space-y-1">
                          <label
                            htmlFor="translation-failure-threshold"
                            className="text-xs text-muted-foreground"
                          >
                            {t("failureThresholdLabel")}
                          </label>
                          <Input
                            id="translation-failure-threshold"
                            className="h-8 w-full text-xs"
                            {...bindNumber(
                              settings.failureThreshold,
                              FAILURE_THRESHOLD_BOUNDS,
                              (value) =>
                                setSettings((prev) => ({
                                  ...prev,
                                  failureThreshold: value,
                                })),
                              t("failureThresholdHint")
                            )}
                          />
                        </div>
                        <div className="space-y-1">
                          <label
                            htmlFor="translation-cooldown-seconds"
                            className="text-xs text-muted-foreground"
                          >
                            {t("cooldownSecondsLabel")}
                          </label>
                          <Input
                            id="translation-cooldown-seconds"
                            className="h-8 w-full text-xs"
                            {...bindNumber(
                              settings.cooldownSeconds,
                              COOLDOWN_SECONDS_BOUNDS,
                              (value) =>
                                setSettings((prev) => ({
                                  ...prev,
                                  cooldownSeconds: value,
                                })),
                              t("cooldownSecondsHint")
                            )}
                          />
                        </div>
                      </PopoverContent>
                    </Popover>
                  </th>
                </tr>
              </thead>
              <tbody className="divide-y divide-border">
                {settings.providers.map((row, index) => {
                  const test = stateForProvider(row.id)
                  // The verdict of the last 测试连接 run, or a quiet dash for
                  // a row that was never tested — a blank is more honest
                  // than a state nobody measured.
                  const stateText =
                    test?.state === "testing"
                      ? t("testStateTesting")
                      : test?.state === "failed"
                        ? t("testStateUnavailable")
                        : test?.state === "ok"
                          ? t("stateOk")
                          : "—"
                  return (
                    <tr
                      key={row.id || index}
                      className={
                        index === editingIndex
                          ? "bg-accent/60"
                          : "hover:bg-accent/30"
                      }
                    >
                      <td className="truncate px-3 py-3">
                        <button
                          type="button"
                          className="block max-w-full truncate text-left font-medium"
                          onClick={() => setEditingIndex(index)}
                          title={row.name || row.baseUrl || undefined}
                        >
                          {row.name ||
                            row.baseUrl ||
                            t("providerNamePlaceholder")}
                        </button>
                      </td>
                      <td className="truncate px-3 py-3 text-muted-foreground">
                        {row.model || "—"}
                      </td>
                      <td className="px-3 py-3">
                        <span
                          title={
                            test?.state === "failed" && test.message
                              ? test.message
                              : undefined
                          }
                          className={`inline-block max-w-full truncate rounded-md border px-2 py-0.5 ${
                            test?.state === "failed"
                              ? "border-destructive/40 bg-destructive/5 text-destructive"
                              : test?.state === "testing"
                                ? "border-border/70 bg-background/60 text-muted-foreground"
                                : test?.state === "ok"
                                  ? "border-border/70 text-muted-foreground"
                                  : "border-transparent text-muted-foreground/60"
                          }`}
                        >
                          {stateText}
                        </span>
                      </td>
                      <td className="px-3 py-3">
                        <span className="flex items-center justify-end gap-0.5">
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-sm"
                            aria-label={t("editProvider")}
                            onClick={() => setEditingIndex(index)}
                          >
                            <Pencil className="size-3.5" />
                          </Button>
                          <Button
                            type="button"
                            variant="ghost"
                            size="icon-sm"
                            aria-label={t("removeProvider")}
                            disabled={settings.providers.length <= 1}
                            onClick={() => removeProvider(index)}
                          >
                            <Trash2 className="size-3.5" />
                          </Button>
                        </span>
                      </td>
                    </tr>
                  )
                })}
              </tbody>
            </table>
            <div className="px-3 py-2">
              <Button
                type="button"
                variant="outline"
                size="sm"
                className="h-7 text-xs"
                onClick={addProvider}
              >
                <Plus className="mr-1 size-3.5" />
                {t("addProvider")}
              </Button>
            </div>
          </SettingCard>

          {provider && (
            <div ref={editorCardRef}>
              <SettingCard className="mt-3 divide-y">
                <div className="flex items-center justify-between gap-2 px-3 py-2">
                  <span className="min-w-0 truncate text-xs font-medium">
                    {t("editProviderTitle", {
                      name:
                        provider.name ||
                        provider.baseUrl ||
                        t("providerNamePlaceholder"),
                    })}
                  </span>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="h-7 shrink-0 text-xs"
                    onClick={closeEditor}
                  >
                    {t("doneEditing")}
                  </Button>
                </div>
                <SettingRow
                  title={t("providerEnabledLabel")}
                  description={t("providerEnabledDescription")}
                  htmlFor="translation-provider-enabled"
                  control={
                    <Switch
                      id="translation-provider-enabled"
                      checked={provider.enabled}
                      onCheckedChange={(checked) =>
                        updateProvider({ enabled: checked })
                      }
                    />
                  }
                />
                <SettingRow
                  title={t("providerNameLabel")}
                  htmlFor="translation-provider-name"
                >
                  <Input
                    id="translation-provider-name"
                    className="h-8 w-full max-w-64 text-xs"
                    value={provider.name ?? ""}
                    onChange={(e) =>
                      updateProvider({ name: e.target.value || null })
                    }
                    placeholder={t("providerNamePlaceholder")}
                  />
                </SettingRow>
                <SettingRow
                  title={t("formatLabel")}
                  htmlFor="translation-api-format"
                  control={
                    <Select
                      value={provider.apiFormat}
                      onValueChange={(value) =>
                        updateProvider({
                          apiFormat: value as TranslationApiFormat,
                        })
                      }
                    >
                      <SelectTrigger
                        id="translation-api-format"
                        className="h-8 w-full max-w-64 text-xs"
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        {API_FORMAT_OPTIONS.map((option) => (
                          <SelectItem key={option.value} value={option.value}>
                            {option.value === "auto"
                              ? t("formatAuto")
                              : option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  }
                />
                <SettingRow
                  title={t("baseUrlLabel")}
                  description={t("baseUrlDescription")}
                  htmlFor="translation-base-url"
                >
                  <Input
                    id="translation-base-url"
                    value={provider.baseUrl}
                    onChange={(e) =>
                      updateProvider({ baseUrl: e.target.value })
                    }
                    placeholder="api.example.com"
                  />
                </SettingRow>
                <SettingRow
                  title={t("apiKeyLabel")}
                  description={t("apiKeyDescription")}
                  htmlFor="translation-api-key"
                >
                  <Input
                    id="translation-api-key"
                    type="password"
                    value={provider.apiKey}
                    onChange={(e) => updateProvider({ apiKey: e.target.value })}
                    placeholder="sk-…"
                  />
                </SettingRow>
                <SettingRow title={t("modelLabel")} htmlFor="translation-model">
                  <div className="flex items-center gap-2">
                    <Input
                      id="translation-model"
                      className="flex-1"
                      value={provider.model}
                      onChange={(e) =>
                        updateProvider({ model: e.target.value })
                      }
                      placeholder={
                        MODEL_PLACEHOLDER_BY_FORMAT[provider.apiFormat]
                      }
                    />
                    {fetchedModels.length > 0 && (
                      <Select
                        value={
                          fetchedModels.includes(provider.model)
                            ? provider.model
                            : ""
                        }
                        onValueChange={(value) =>
                          updateProvider({ model: value })
                        }
                      >
                        <SelectTrigger
                          className="h-8 w-44 shrink-0 text-xs"
                          aria-label={t("modelPicker")}
                        >
                          <SelectValue placeholder={t("modelPicker")} />
                        </SelectTrigger>
                        <SelectContent className="max-h-64">
                          {fetchedModels.map((model) => (
                            <SelectItem key={model} value={model}>
                              {model}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    )}
                    <Button
                      type="button"
                      variant="outline"
                      size="icon-sm"
                      onClick={() => void handleFetchModels()}
                      disabled={!canProbeModels(provider) || fetchingModels}
                      aria-label={t("fetchModels")}
                    >
                      {fetchingModels ? (
                        <Loader2 className="size-4 animate-spin" />
                      ) : (
                        <RefreshCw className="size-4" />
                      )}
                    </Button>
                  </div>
                  {showEmptyHint && (
                    <p className="text-xs text-muted-foreground">
                      {t("fetchModelsEmpty")}
                    </p>
                  )}
                </SettingRow>
                <SettingRow
                  title={t("rpmCapLabel")}
                  description={t("rpmCapDescription")}
                  htmlFor="translation-rpm-cap"
                >
                  <div className="flex items-center gap-1">
                    <FieldHelp
                      title={t("helpRpmTitle")}
                      body={t("helpRpmBody")}
                      label={t("rpmCapLabel")}
                    />
                    <Input
                      id="translation-rpm-cap"
                      className="h-8 w-44 shrink-0 text-xs"
                      {...bindNumber(
                        provider.rpmCap,
                        RPM_CAP_BOUNDS,
                        (value) => updateProvider({ rpmCap: value }),
                        t("rpmCapDefaultHint")
                      )}
                    />
                  </div>
                </SettingRow>
              </SettingCard>
            </div>
          )}
        </SettingsSection>

        {/*
          Call statistics: one line of session counters for the configured
          endpoint — dispatch volume, gate rejections, outright failures,
          mean latency. In-memory only; the numbers reset on restart.
        */}
        <SettingsSection title={t("statsTitle")}>
          <SettingCard className="p-3">
            <span className="text-xs text-muted-foreground">
              {stats
                ? t("statsLine", {
                    requests: stats.requests,
                    ok: stats.ok,
                    rejected: stats.rejected,
                    failures: stats.failures,
                    latency:
                      stats.avgLatencyMs > 0
                        ? Math.round(stats.avgLatencyMs)
                        : "—",
                  })
                : t("statsEmpty")}
            </span>
          </SettingCard>
        </SettingsSection>

        <div className="flex items-center justify-end gap-2 pt-1">
          {hasUnsavedChanges && (
            <span className="mr-auto text-xs text-amber-600 dark:text-amber-400">
              {t("unsavedChanges")}
            </span>
          )}
          <Button
            type="button"
            size="sm"
            variant="outline"
            onClick={handleTestConnection}
            disabled={testing || settings.providers.length === 0}
          >
            {testing ? (
              <>
                <Loader2
                  className="h-3.5 w-3.5 animate-spin"
                  aria-hidden="true"
                />
                {t("testingLabel")}
              </>
            ) : (
              t("testLabel")
            )}
          </Button>
          <Button
            type="button"
            size="sm"
            onClick={handleSave}
            disabled={saving}
          >
            {saving ? (
              <>
                <Loader2
                  className="h-3.5 w-3.5 animate-spin"
                  aria-hidden="true"
                />
                {t("savingLabel")}
              </>
            ) : (
              t("saveLabel")
            )}
          </Button>
        </div>
      </section>
    </ScrollArea>
  )
}

/** The question-mark badge a numeric field carries: clicking it opens a
 * popover that says what the knob does, how to pick a value, and what the
 * recommended range is — the description line stays one line. */
function FieldHelp({
  title,
  body,
  label,
}: {
  title: string
  body: string
  label: string
}) {
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button
          type="button"
          variant="ghost"
          size="icon-sm"
          className="text-muted-foreground hover:text-foreground"
          aria-label={label}
        >
          <HelpCircle className="size-3.5" />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-80">
        <p className="text-xs font-medium">{title}</p>
        <p className="mt-1 whitespace-pre-line text-xs leading-5 text-muted-foreground">
          {body}
        </p>
      </PopoverContent>
    </Popover>
  )
}
