"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import {
  HelpCircle,
  Languages,
  Loader2,
  Pencil,
  Plus,
  RefreshCw,
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
  clearTranslationCache,
  getTranslationCacheStats,
  getTranslationMetrics,
  getTranslationPoolStatus,
  getTranslationSettings,
  listTranslationModels,
  testTranslationSettings,
  updateTranslationSettings,
} from "@/lib/api"
import { subscribe } from "@/lib/platform"
import { APP_LOCALES, toIntlLocale } from "@/lib/i18n"
import { formatBytes } from "@/lib/format-bytes"
import { toErrorMessage } from "@/lib/app-error"
import type {
  AppLocale,
  TranslationApiFormat,
  TranslationCacheStats,
  TranslationMetricsSnapshot,
  TranslationPoolStatus,
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

/**
 * A new pool row's identity. Generated client-side so the "test connection"
 * and "fetch models" buttons can aim at the exact row being edited even
 * before the first save; the backend keeps the id on save.
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
    settings.translateThinking,
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
 * Ollama, which serves locally with none (`validate` in
 * `src-tauri/src/translation/settings.rs`). Under `auto` the dialect is read
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
  const afterScheme = baseUrl.trim().split("://").pop() ?? ""
  const hostAndPort = (afterScheme.split(/[/?#]/)[0] ?? "").toLowerCase()
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

/** The shared column grid for the provider table: header row and data rows
 * both carry it, so 供应商 / 模型 / 当前速率 / 状态 line up exactly. The
 * rate, state, and action tracks are FIXED rem widths — auto tracks size
 * per grid, and each row is its own grid, so content-sized columns would
 * drift out from under their headers row by row. */
const PROVIDER_GRID_COLS =
  "grid-cols-[minmax(0,1.15fr)_minmax(0,0.95fr)_7.5rem_9rem_3.25rem]"

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
 * keys. The backend speaks English constants (see the `ERR_*` strings in
 * `src-tauri/src/translation/settings.rs`); the map is exact-match, so any
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
    translateThinking: false,
    apiFormat: "auto",
    selectionTranslate: true,
    selectionTargetLang: null,
    toggleAlwaysVisible: false,
    batchMaxChars: null,
    carryContext: true,
  })
  /**
   * Which pool row the editor card is open for, or `null` when the card is
   * collapsed — the list alone reads much cleaner, and the endpoint fields
   * only matter while adding or editing one row.
   */
  const [editingIndex, setEditingIndex] = useState<number | null>(null)
  const [targetLang, setTargetLang] = useState<string>("__interface__")
  const [cacheStats, setCacheStats] = useState<TranslationCacheStats>({
    memoryEntries: 0,
    diskEntries: 0,
    diskBytes: 0,
  })
  const [poolStatus, setPoolStatus] = useState<TranslationPoolStatus[]>([])
  const [metrics, setMetrics] = useState<TranslationMetricsSnapshot | null>(
    null
  )
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
   * override the pool's derived state in the 状态 column — a fresh draft row
   * has no pool state at all, and "the test just failed" must survive the
   * next status poll regardless of what the limiter thinks.
   */
  const [providerTestState, setProviderTestState] = useState<
    Record<string, { state: "testing" | "ok" | "failed"; message?: string }>
  >({})

  const loadCacheStats = useCallback(async () => {
    try {
      setCacheStats(await getTranslationCacheStats())
    } catch {
      // The stats are informational; a failure must not block the page.
    }
  }, [])

  useEffect(() => {
    let active = true
    getTranslationSettings()
      .then((stored) => {
        if (!active) return
        storedRef.current = stored
        // A fresh install (or a legacy disabled row with no list) gets one
        // empty draft to fill — and the editor opens on it, since there is
        // nothing else on the card to look at. Otherwise the card stays
        // collapsed behind the list until the user edits a row.
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
    loadCacheStats()
    return () => {
      active = false
    }
  }, [t, loadCacheStats])

  // Pool status strip above the list: the adaptive rates, cooldowns, and
  // session disables the runtime learns. Saved settings only — an unsaved
  // draft has no pool. Push, not poll: the backend emits
  // `translation-pool-changed` on every observable limiter change and this
  // page re-fetches immediately. Two backstops around it — a slow safety
  // poll (cooldowns expire by the clock, not by requests) and a local
  // one-second countdown that ticks a parked member's remaining window
  // between events without touching the backend.
  useEffect(() => {
    if (loading || !settings.enabled) return
    let active = true
    const refetch = () => {
      getTranslationPoolStatus()
        .then((status) => {
          if (active) setPoolStatus(status)
        })
        .catch(() => {
          // Informational; the badges simply stay as they were.
        })
      // The counters ride the same refresh: dispatch volume and gate
      // rejections move with the same traffic the limiter reacts to.
      getTranslationMetrics()
        .then((snapshot) => {
          if (active) setMetrics(snapshot)
        })
        .catch(() => {
          // Informational; the summary line simply stays as it was.
        })
    }
    refetch()
    let unsubscribe: (() => void) | null = null
    void subscribe("translation-pool-changed", () => refetch()).then((un) => {
      if (active) {
        unsubscribe = un
      } else {
        un()
      }
    })
    const safety = window.setInterval(refetch, 60_000)
    const countdown = window.setInterval(() => {
      setPoolStatus((prev) => {
        if (!prev.some((entry) => entry.cooldownRemainingMs > 0)) return prev
        return prev.map((entry) =>
          entry.cooldownRemainingMs > 0
            ? {
                ...entry,
                cooldownRemainingMs: Math.max(
                  0,
                  entry.cooldownRemainingMs - 1000
                ),
              }
            : entry
        )
      })
    }, 1_000)
    return () => {
      active = false
      unsubscribe?.()
      window.clearInterval(safety)
      window.clearInterval(countdown)
    }
  }, [loading, settings.enabled, savedSnapshot])

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
      // legacy flat fields on save, which the pool-aware form no longer
      // edits. Mirrors the old single-endpoint page's immovable fields.
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
    // Every provider in the pool gets its own English test sentence, in
    // parallel; the 状态 column turns 正常 or 不可用 per endpoint as the
    // results land. The backend feeds each outcome into that provider's
    // adaptive limiter too, so a failing endpoint starts throttled and a
    // 401-shaped one is two strikes from session-disabled.
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

  const handleClearCache = useCallback(async () => {
    try {
      const stats = await clearTranslationCache()
      setCacheStats(stats)
      toast.success(t("clearCacheDone"))
    } catch (err) {
      toast.error(localizeBackendError(err))
    }
  }, [t, localizeBackendError])

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

  const statusForProvider = (id: string | undefined) =>
    id ? poolStatus.find((entry) => entry.id === id) : undefined

  const describePoolStatus = (status: TranslationPoolStatus): string => {
    if (status.disabledReason) {
      return t("poolDisabled", { reason: status.disabledReason })
    }
    if (status.cooldownRemainingMs > 0) {
      return t("poolCooldown", {
        seconds: Math.ceil(status.cooldownRemainingMs / 1000),
        rpm: Math.round(status.allowedRpm),
      })
    }
    if (status.allowedRpm > 0) {
      return t("poolRate", { rpm: Math.round(status.allowedRpm) })
    }
    return t("poolIdle")
  }

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
            <SettingRow
              title={t("translateThinkingLabel")}
              description={t("translateThinkingDescription")}
              htmlFor="translation-thinking"
              control={
                <Switch
                  id="translation-thinking"
                  checked={settings.translateThinking}
                  onCheckedChange={(checked) =>
                    setSettings((prev) => ({
                      ...prev,
                      translateThinking: checked,
                    }))
                  }
                />
              }
            />
            <SettingRow
              title={t("selectionTranslateLabel")}
              description={t("selectionTranslateDescription")}
              htmlFor="translation-selection"
              control={
                <Switch
                  id="translation-selection"
                  checked={settings.selectionTranslate}
                  onCheckedChange={(checked) =>
                    setSettings((prev) => ({
                      ...prev,
                      selectionTranslate: checked,
                    }))
                  }
                />
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
            {metrics && (
              // The session-wide counters: this is the line that answers
              // "is anything translating, and what is the endpoint doing to
              // my chunks" without opening the logs.
              <div className="px-3 py-2 text-xs text-muted-foreground">
                {t("metricsSummary", {
                  dispatched: metrics.dispatchedTotal,
                  served: metrics.servedTotal,
                  cacheHits: metrics.cacheHits,
                  rejected: metrics.gateRejectedTotal,
                  invented: metrics.gateRejectedInvented,
                  echo: metrics.gateRejectedEcho,
                  dropped: metrics.gateRejectedDroppedNumbers,
                  truncated: metrics.truncatedTotal,
                })}
              </div>
            )}
            {/*
              One shared grid so the column headers and every data row align:
              供应商 | 模型 | 当前速率 | 状态 | (行操作). The rate/state/action
              tracks are fixed rem widths (see PROVIDER_GRID_COLS) — auto
              tracks size per grid and each row is its own grid, so headers
              would drift off their columns row by row. Headers and values
              share left edges inside their tracks.
            */}
            <div
              className={`grid items-center gap-3 bg-muted/30 px-3 py-2.5 text-xs text-muted-foreground ${PROVIDER_GRID_COLS}`}
            >
              <span className="min-w-0 truncate">{t("colProvider")}</span>
              <span className="min-w-0 truncate">{t("colModel")}</span>
              {/* The badge cells carry px-1.5 of their own, so the headers
                  over them take the same inset and the text lines align. */}
              <span className="min-w-0 truncate pl-2">{t("colRate")}</span>
              <span className="min-w-0 truncate pl-2">{t("colState")}</span>
              <span aria-hidden="true" />
            </div>
            {settings.providers.map((row, index) => {
              const entry = statusForProvider(row.id)
              const disabled = Boolean(entry?.disabledReason)
              const cooling = !disabled && (entry?.cooldownRemainingMs ?? 0) > 0
              // A member below the health threshold only receives fallback
              // (or probe) traffic; the amber state badge is how the reader
              // learns their relay is quietly refusing translations.
              const degraded =
                !disabled && !cooling && (entry?.health?.degraded ?? false)
              const test = providerTestState[row.id]
              // The test verdict wins while it exists: a fresh draft row has
              // no pool state at all, and "the test just failed" must survive
              // the next status refresh regardless of limiter side effects.
              const testing = test?.state === "testing"
              const failed = test?.state === "failed"
              const testedOk = test?.state === "ok"
              const stateText = testing
                ? t("testStateTesting")
                : failed
                  ? t("testStateUnavailable")
                  : testedOk
                    ? t("poolStateOk")
                    : disabled
                      ? t("poolDisabledShort")
                      : cooling
                        ? t("poolCooldownShort", {
                            seconds: Math.ceil(
                              (entry?.cooldownRemainingMs ?? 0) / 1000
                            ),
                          })
                        : degraded
                          ? t("poolStateDegraded")
                          : (entry?.allowedRpm ?? 0) > 0
                            ? t("poolStateOk")
                            : t("poolIdle")
              // The hover hint answers "why is it amber": a failed test shows
              // the endpoint's own words; otherwise the health breakdown.
              const stateTitle =
                failed && test?.message
                  ? test.message
                  : entry?.health && !entry.health.observing
                    ? t("poolHealth", {
                        score: Math.round(entry.health.score),
                        quality: Math.round(entry.health.quality * 100),
                        stability: Math.round(entry.health.stability * 100),
                        speed: Math.round(entry.health.speed * 100),
                        sample: entry.health.sample,
                      })
                    : undefined
              return (
                <div
                  key={row.id || index}
                  className={`grid items-center gap-3 px-3 py-3 text-xs ${
                    index === editingIndex
                      ? "bg-accent/60"
                      : "hover:bg-accent/30"
                  } ${PROVIDER_GRID_COLS}`}
                >
                  <button
                    type="button"
                    className="min-w-0 truncate text-left font-medium"
                    onClick={() => setEditingIndex(index)}
                    title={row.name || row.baseUrl || undefined}
                  >
                    {row.name || row.baseUrl || t("providerNamePlaceholder")}
                  </button>
                  <span className="min-w-0 truncate text-muted-foreground">
                    {row.model || "—"}
                  </span>
                  <span className="min-w-0">
                    {!disabled && (entry?.allowedRpm ?? 0) > 0 ? (
                      <span
                        // The allowed rate and the served rate are different
                        // facts: the limiter may grant 21/min while the lane
                        // caps and the endpoint's own latency deliver far
                        // less. "High rate but nothing translates" reports
                        // read the second number.
                        title={t("poolRateHint", {
                          rpm: Math.round(entry!.allowedRpm),
                          count: entry?.dispatchedLastMinute ?? 0,
                        })}
                        className="inline-block max-w-full truncate rounded-md border border-border/70 bg-background/60 px-2 py-0.5 font-medium text-foreground/80"
                      >
                        {t("poolRateShort", {
                          rpm: Math.round(entry!.allowedRpm),
                        })}
                        {(entry?.dispatchedLastMinute ?? 0) > 0
                          ? ` · ${t("poolDispatchShort", {
                              count: entry!.dispatchedLastMinute,
                            })}`
                          : ""}
                      </span>
                    ) : (
                      <span className="text-muted-foreground/60">—</span>
                    )}
                  </span>
                  <span className="min-w-0">
                    <span
                      title={stateTitle}
                      className={`inline-block max-w-full truncate rounded-md border px-2 py-0.5 ${
                        failed || disabled
                          ? "border-destructive/40 bg-destructive/5 text-destructive"
                          : testing
                            ? "border-border/70 bg-background/60 text-muted-foreground"
                            : cooling || degraded
                              ? "border-amber-500/40 bg-amber-500/5 text-amber-600 dark:text-amber-400"
                              : "border-border/70 text-muted-foreground"
                      }`}
                    >
                      {stateText}
                    </span>
                  </span>
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
                </div>
              )
            })}
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
                {statusForProvider(provider.id) && (
                  <div className="space-y-1 px-3 py-2 text-xs text-muted-foreground">
                    <PoolStatusLine
                      status={statusForProvider(provider.id)!}
                      describe={describePoolStatus}
                    />
                    <PoolHealthLine status={statusForProvider(provider.id)!} />
                  </div>
                )}
              </SettingCard>
            </div>
          )}
        </SettingsSection>

        <SettingsSection title={t("cacheTitle")}>
          <SettingCard className="p-3">
            <div className="flex items-center justify-between gap-3">
              <span className="text-xs text-muted-foreground">
                {t("cacheStats", {
                  memory: cacheStats.memoryEntries,
                  disk: cacheStats.diskEntries,
                  size: formatBytes(cacheStats.diskBytes),
                })}
              </span>
              <Button
                variant="outline"
                size="sm"
                className="h-8 text-xs"
                onClick={handleClearCache}
              >
                <Trash2 className="h-3.5 w-3.5 mr-1" />
                {t("clearCache")}
              </Button>
            </div>
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

/** One line summarizing a pool member's runtime state. The description text
 * comes in as a callback so this stays a plain function — threading the
 * translator's type through here instantiates nothing but pain. */
function PoolStatusLine({
  status,
  describe,
}: {
  status: TranslationPoolStatus
  describe: (status: TranslationPoolStatus) => string
}) {
  return <span>{describe(status)}</span>
}

/** The health-score line under the pool status: sub-score percentages, the
 * sample it judged, and the degraded verdict. Nothing renders while the
 * member is unobserved — a blank is more honest than a made-up 70. */
function PoolHealthLine({ status }: { status: TranslationPoolStatus }) {
  const t = useTranslations("TranslationSettings")
  const health = status.health
  if (!health) return null
  if (health.observing) {
    return <div>{t("poolHealthObserving", { sample: health.sample })}</div>
  }
  return (
    <div>
      {t("poolHealthLine", {
        score: Math.round(health.score),
        quality: Math.round(health.quality * 100),
        stability: Math.round(health.stability * 100),
        speed: Math.round(health.speed * 100),
        sample: health.sample,
      })}
      {health.degraded ? ` — ${t("poolHealthDegradedNote")}` : ""}
    </div>
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

/** Module-level stand-in for the aria-label so `FieldHelp` stays a plain
 * function; the visible popover title already names the field. */
