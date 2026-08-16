"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Eye,
  EyeOff,
  Loader2,
  Plus,
  RefreshCw,
  Save,
  ShieldCheck,
  Trash2,
} from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxInput,
  ComboboxItem,
  ComboboxList,
} from "@/components/ui/combobox"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Textarea } from "@/components/ui/textarea"
import {
  acpCursorAuthStatus,
  acpCursorListModels,
  acpUpdateAgentConfig,
} from "@/lib/api"
import type { AcpAgentInfo, CursorAuthStatus } from "@/lib/types"
import { cn } from "@/lib/utils"

const CURSOR_API_KEY_ENV = "CURSOR_API_KEY"
const CURSOR_API_BASE_URL_ENV = "CURSOR_API_BASE_URL"
const CURSOR_MODEL_ENV = "CURSOR_MODEL"
/** codeg-side launch knob: "1" inserts the CLI's root `--force` flag (Run
 * Everything) before the `acp` subcommand. The CLI reads no such env var. */
const CURSOR_FORCE_ENV = "CURSOR_FORCE"
/** codeg-side knob recording the chosen authentication method. Read by the
 * launch path (`apply_cursor_env_policy`): in `subscription` mode it clears any
 * inherited CURSOR_API_KEY/BASE_URL so the CLI uses the browser-login
 * credential. The CLI itself ignores this var. */
const CURSOR_AUTH_MODE_ENV = "CURSOR_AUTH_MODE"

const UNSET = "__unset__"

/** The Cursor CLI's two authentication modes. `custom` keeps the explicit
 * account key and optional endpoint selected by the user. */
export type CursorAuthMethod = "subscription" | "custom"

export function normalizeCursorEndpoint(value: string): string {
  const trimmed = value.trim()
  if (!trimmed) return ""
  try {
    // Validation only: Cursor receives the user's exact trimmed endpoint. URL
    // serialization would otherwise rewrite path/query/fragment semantics.
    new URL(trimmed)
  } catch {
    throw new Error("Invalid Cursor endpoint")
  }
  return trimmed
}

/** One entry in the model picker. `value` is the `--model` id, `label` the
 * human name from `cursor-agent models`. base-ui's Combobox uses the `{ value,
 * label }` shape automatically (label for display + filtering, value for the
 * value); `isDefault` only drives our own badge. */
type CursorModelItem = { value: string; label: string; isDefault: boolean }

/**
 * Build the env map to persist for Cursor. The authentication method decides
 * which credential is written:
 *  - `subscription` — browser login only; the API key is deleted so a launch
 *    (and the probes) fall back to the Cursor account.
 *  - `custom` — the Cursor API key and optional endpoint are written from the
 *    form.
 * The default model and the Run Everything (`--force`) knob apply to both.
 * `CURSOR_AUTH_MODE` is always recorded, and unrelated keys are preserved.
 */
export function buildCursorEnv(
  prevEnv: Record<string, string>,
  mode: CursorAuthMethod,
  apiKey: string,
  baseUrl: string,
  model: string,
  force: boolean
): Record<string, string> {
  const env: Record<string, string> = { ...prevEnv }
  const setOrDelete = (key: string, value: string) => {
    const trimmed = value.trim()
    if (trimmed) {
      env[key] = trimmed
    } else {
      delete env[key]
    }
  }
  env[CURSOR_AUTH_MODE_ENV] = mode
  if (mode === "custom") {
    setOrDelete(CURSOR_API_KEY_ENV, apiKey)
    setOrDelete(CURSOR_API_BASE_URL_ENV, normalizeCursorEndpoint(baseUrl))
  } else {
    // Subscription: never ship a saved key — the launch policy additionally
    // strips any inherited one so browser login is used.
    delete env[CURSOR_API_KEY_ENV]
    delete env[CURSOR_API_BASE_URL_ENV]
  }
  setOrDelete(CURSOR_MODEL_ENV, model)
  setOrDelete(CURSOR_FORCE_ENV, force ? "1" : "")
  return env
}

/** Resolve the persisted authentication method, tolerant of legacy rows: an
 * explicit `CURSOR_AUTH_MODE` wins; otherwise a saved API key implies custom. */
export function inferCursorMode(env: Record<string, string>): CursorAuthMethod {
  const explicit = (env[CURSOR_AUTH_MODE_ENV] ?? "").trim()
  if (explicit === "subscription" || explicit === "custom") return explicit
  return (env[CURSOR_API_KEY_ENV] ?? "").trim() ? "custom" : "subscription"
}

/** The copy-pasteable login command. The managed cursor-agent binary lives in
 * codeg's cache (not on PATH), so a bare `cursor-agent login` fails — use the
 * resolved absolute path, quoted when it contains whitespace. */
export function cursorLoginCommand(binaryPath?: string | null): string {
  const path = (binaryPath ?? "").trim()
  if (!path) return "cursor-agent login"
  const program = /\s/.test(path) ? `"${path}"` : path
  return `${program} login`
}

/** The saved env's Run Everything knob, tolerant of hand-edited values. */
export function isCursorForceEnabled(env: Record<string, string>): boolean {
  const value = (env[CURSOR_FORCE_ENV] ?? "").trim().toLowerCase()
  return value === "1" || value === "true"
}

/** One editable permission-rule row list (allow or deny). */
function RuleListEditor({
  rules,
  onChange,
  placeholder,
  addLabel,
  disabled,
  tone,
}: {
  rules: string[]
  onChange: (rules: string[]) => void
  placeholder: string
  addLabel: string
  disabled: boolean
  tone: "allow" | "deny"
}) {
  return (
    <div className="space-y-1.5">
      {rules.map((rule, index) => (
        <div className="flex items-center gap-1.5" key={index}>
          <Input
            className={cn(
              "h-7 flex-1 font-mono text-xs",
              tone === "deny" && "border-destructive/40"
            )}
            disabled={disabled}
            onChange={(e) => {
              const next = [...rules]
              next[index] = e.target.value
              onChange(next)
            }}
            placeholder={placeholder}
            value={rule}
          />
          <Button
            className="h-7 w-7 shrink-0 text-muted-foreground hover:text-destructive"
            disabled={disabled}
            onClick={() => onChange(rules.filter((_, i) => i !== index))}
            size="icon"
            type="button"
            variant="ghost"
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </div>
      ))}
      <Button
        className="h-7 gap-1 px-2 text-xs"
        disabled={disabled}
        onClick={() => onChange([...rules, ""])}
        size="sm"
        type="button"
        variant="outline"
      >
        <Plus className="h-3 w-3" />
        {addLabel}
      </Button>
    </div>
  )
}

/**
 * Dedicated settings panel for Cursor (cursor-agent CLI). The user first picks
 * an **authentication method** (a Select, matching the Codex panel's idiom).
 * Both methods authenticate against Cursor's own backend:
 *
 * 1. **Official subscription** — sign in with a Cursor account
 *    (`"<path>" login`). No credential is stored; the launch clears any
 *    inherited CURSOR_API_KEY so browser login is used.
 * 2. **Cursor API key** — a Cursor Dashboard account key for headless/server
 *    machines (CURSOR_API_KEY), with an optional explicit
 *    CURSOR_API_BASE_URL for compatible deployments.
 *
 * The model picker (a searchable Combobox of `cursor-agent models`) is shared
 * and only shown once real models were fetched; the chosen id is stored as
 * CURSOR_MODEL and passed to the CLI as its root `--model` flag at launch.
 * The permission/sandbox editor and the raw-JSON advanced card are unchanged.
 */
export function CursorConfigPanel({
  agent,
  saving,
  onSaveEnv,
  onSaved,
  onAffectedSessions,
}: {
  agent: AcpAgentInfo
  saving: boolean
  onSaveEnv: (env: Record<string, string>, enabled: boolean) => Promise<unknown>
  onSaved: () => void
  /** Reports how many running sessions a cli-config.json write marked
   * restart-required (the env step reports its own count internally). */
  onAffectedSessions: (count: number) => void
}) {
  const t = useTranslations("AcpAgentSettings")
  const cursorProbeError = useCallback(
    (code?: string | null) => {
      switch (code) {
        case "cursor_probe_timeout":
          return t("cursor.probeTimeout")
        case "cursor_probe_output_too_large":
          return t("cursor.probeOutputTooLarge")
        case "cursor_probe_nonzero_exit":
          return t("cursor.probeNonzeroExit")
        case "cursor_probe_invalid_utf8":
        case "cursor_probe_empty_output":
        case "cursor_probe_invalid_format":
          return t("cursor.probeInvalidOutput")
        case "cursor_probe_invalid_endpoint":
          return t("cursor.baseUrlInvalid")
        default:
          return t("cursor.probeFailed")
      }
    },
    [t]
  )

  // --- authentication method ---
  const [mode, setMode] = useState<CursorAuthMethod>(() =>
    inferCursorMode(agent.env)
  )

  // --- auth card state ---
  const [auth, setAuth] = useState<CursorAuthStatus | null>(null)
  const [authLoading, setAuthLoading] = useState(false)
  const [copied, setCopied] = useState(false)

  // --- credential state (custom / API key mode) ---
  const [apiKey, setApiKey] = useState(
    () => agent.env[CURSOR_API_KEY_ENV] ?? ""
  )
  const [baseUrl, setBaseUrl] = useState(
    () => agent.env[CURSOR_API_BASE_URL_ENV] ?? ""
  )
  const authoritativeCredentialsRef = useRef({
    mode: inferCursorMode(agent.env),
    apiKey: agent.env[CURSOR_API_KEY_ENV] ?? "",
    baseUrl: agent.env[CURSOR_API_BASE_URL_ENV] ?? "",
  })
  const [showKey, setShowKey] = useState(false)

  // A parent refresh may replace the authoritative env without remounting this
  // panel. Reconcile only a clean draft; unsaved edits remain untouched. A
  // successful save explicitly reconciles below, including credential removal.
  useEffect(() => {
    const previous = authoritativeCredentialsRef.current
    const next = {
      mode: inferCursorMode(agent.env),
      apiKey: agent.env[CURSOR_API_KEY_ENV] ?? "",
      baseUrl: agent.env[CURSOR_API_BASE_URL_ENV] ?? "",
    }
    const draftWasClean =
      mode === previous.mode &&
      apiKey === previous.apiKey &&
      baseUrl === previous.baseUrl
    authoritativeCredentialsRef.current = next
    if (draftWasClean) {
      setMode(next.mode)
      setApiKey(next.apiKey)
      setBaseUrl(next.baseUrl)
    }
  }, [agent.env, apiKey, baseUrl, mode])

  // --- model state (searchable picker over cursor-agent models) ---
  const [model, setModel] = useState(() => agent.env[CURSOR_MODEL_ENV] ?? "")
  const [models, setModels] = useState<CursorModelItem[]>([])
  const [modelsError, setModelsError] = useState<string | null>(null)
  const [modelsLoading, setModelsLoading] = useState(false)
  const [modelsLoaded, setModelsLoaded] = useState(false)

  // --- permissions card state ---
  const settings = agent.cursor_settings
  // Default a fresh Cursor agent to Run Everything (--force): only when the
  // CURSOR_FORCE knob was never set does it default on; an explicit "0" (the
  // user chose "Ask before running") is respected.
  const [force, setForce] = useState(() =>
    CURSOR_FORCE_ENV in agent.env ? isCursorForceEnabled(agent.env) : true
  )
  const [sandboxMode, setSandboxMode] = useState(
    () => settings?.sandbox_mode ?? ""
  )
  const [allowRules, setAllowRules] = useState<string[]>(
    () => settings?.permissions_allow ?? []
  )
  const [denyRules, setDenyRules] = useState<string[]>(
    () => settings?.permissions_deny ?? []
  )

  // --- unified save state (auth method + model + permissions) ---
  const [savingAll, setSavingAll] = useState(false)

  // --- advanced card state ---
  const [advancedOpen, setAdvancedOpen] = useState(false)
  const [rawConfig, setRawConfig] = useState(
    () => agent.cursor_cli_config_json ?? ""
  )
  const [savingRaw, setSavingRaw] = useState(false)

  const mountedRef = useRef(true)
  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])

  // The effective API key the probes should test: the typed key in API-key
  // mode, or an empty string in subscription mode (which forces the
  // browser-login credential and strips any inherited CURSOR_API_KEY). Kept in
  // a ref so the probe callbacks stay stable and don't re-fire on keystroke.
  const probeKeyRef = useRef("")
  const probeBaseUrlRef = useRef("")
  useEffect(() => {
    probeKeyRef.current = mode === "custom" ? apiKey : ""
    probeBaseUrlRef.current = mode === "custom" ? baseUrl : ""
  }, [mode, apiKey, baseUrl])

  const refreshAuth = useCallback(async () => {
    setAuthLoading(true)
    try {
      const status = await acpCursorAuthStatus(
        probeKeyRef.current,
        probeBaseUrlRef.current
      )
      if (mountedRef.current) setAuth(status)
    } catch {
      // Probe failures already surface through `auth.error`; a transport-level
      // failure just leaves the card in its unknown state.
    } finally {
      if (mountedRef.current) setAuthLoading(false)
    }
  }, [])

  // Probe on mount and whenever the method changes (the ref above is updated
  // first, so the probe sees the right credential for the new mode).
  useEffect(() => {
    void refreshAuth()
  }, [refreshAuth, mode])

  const loadModels = useCallback(async () => {
    setModelsLoading(true)
    setModelsError(null)
    try {
      const result = await acpCursorListModels(
        probeKeyRef.current,
        probeBaseUrlRef.current
      )
      if (!mountedRef.current) return
      setModels(
        result.models.map((m) => ({
          value: m.id,
          label: m.label || m.id,
          isDefault: m.is_default,
        }))
      )
      setModelsError(result.error ? cursorProbeError(result.error_code) : null)
      setModelsLoaded(true)
    } catch {
      if (mountedRef.current) {
        setModelsError(cursorProbeError())
        setModelsLoaded(true)
      }
    } finally {
      if (mountedRef.current) setModelsLoading(false)
    }
  }, [cursorProbeError])

  const authState: "loading" | "missing" | "ok" | "unauthenticated" =
    authLoading && !auth
      ? "loading"
      : !auth || !auth.installed
        ? "missing"
        : auth.is_authenticated
          ? "ok"
          : "unauthenticated"

  // Once the account reports authenticated (either mode), fetch the model list
  // instead of waiting for a manual "load" click. Re-fetch when the method
  // changes so an API key and a browser login can list different catalogs.
  useEffect(() => {
    if (authState !== "ok") return
    if (modelsLoaded || modelsLoading) return
    void loadModels()
  }, [authState, loadModels, modelsLoaded, modelsLoading])

  // Reset the "loaded" latch when the method changes so the effect above
  // re-probes the model list for the newly-selected credential.
  useEffect(() => {
    setModelsLoaded(false)
    setModels([])
    setModelsError(null)
  }, [mode])

  const loginCommand = cursorLoginCommand(auth?.binary_path)

  const copyLoginCommand = useCallback(async () => {
    try {
      await navigator.clipboard.writeText(loginCommand)
      setCopied(true)
      setTimeout(() => setCopied(false), 1500)
    } catch {
      // Clipboard may be unavailable (permissions); the command stays visible.
    }
  }, [loginCommand])

  // Model picker items: the fetched catalog, plus a saved-but-unlisted model
  // kept as its own entry so a hand-set value still displays and isn't lost.
  const modelItems = useMemo<CursorModelItem[]>(() => {
    if (model && !models.some((m) => m.value === model)) {
      return [{ value: model, label: model, isDefault: false }, ...models]
    }
    return models
  }, [models, model])
  const selectedModelItem = modelItems.find((m) => m.value === model) ?? null

  const handleSelectModel = useCallback((item: CursorModelItem | null) => {
    setModel(item?.value ?? "")
  }, [])

  /** One save for auth method + model (env) and permissions (cli-config.json). */
  const saveAll = useCallback(async () => {
    if (mode === "custom" && !apiKey.trim()) {
      toast.error(t("cursor.customApiKeyRequired"))
      return
    }
    const prevEnv = agent.env
    let nextEnv: Record<string, string>
    try {
      nextEnv = buildCursorEnv(prevEnv, mode, apiKey, baseUrl, model, force)
    } catch {
      toast.error(t("cursor.baseUrlInvalid"))
      return
    }
    setSavingAll(true)
    try {
      await onSaveEnv(nextEnv, agent.enabled)
      try {
        const affected = await acpUpdateAgentConfig(agent.agent_type, {
          cursor_structured: {
            sandboxMode,
            permissionsAllow: allowRules,
            permissionsDeny: denyRules,
          },
        })
        onAffectedSessions(affected)
      } catch (e) {
        // A failed rules write must not leave the freshly-saved permission
        // knobs behind — e.g. Run Everything enabled while the new deny
        // rules never landed. Put the env back exactly as it was; if even
        // the rollback fails the error toast below still fires.
        await onSaveEnv(prevEnv, agent.enabled).catch(() => {})
        throw e
      }
      const savedCredentials = {
        mode: inferCursorMode(nextEnv),
        apiKey: nextEnv[CURSOR_API_KEY_ENV] ?? "",
        baseUrl: nextEnv[CURSOR_API_BASE_URL_ENV] ?? "",
      }
      authoritativeCredentialsRef.current = savedCredentials
      setMode(savedCredentials.mode)
      setApiKey(savedCredentials.apiKey)
      setBaseUrl(savedCredentials.baseUrl)
      toast.success(t("toasts.cursorSaved"))
      onSaved()
    } catch {
      // Save transports may include request context in their diagnostic. Never
      // reflect it into the DOM because this form contains an API key.
      toast.error(t("toasts.saveCursorConfigFailed"))
    } finally {
      if (mountedRef.current) setSavingAll(false)
    }
  }, [
    agent.agent_type,
    agent.enabled,
    agent.env,
    allowRules,
    apiKey,
    baseUrl,
    denyRules,
    force,
    mode,
    model,
    onAffectedSessions,
    onSaveEnv,
    onSaved,
    sandboxMode,
    t,
  ])

  const saveRaw = useCallback(async () => {
    setSavingRaw(true)
    try {
      const affected = await acpUpdateAgentConfig(agent.agent_type, {
        cursor_cli_config_json: rawConfig,
      })
      onAffectedSessions(affected)
      toast.success(t("toasts.cursorSaved"))
      onSaved()
    } catch (e) {
      toast.error(
        `${t("toasts.saveCursorConfigFailed")}: ${
          e instanceof Error ? e.message : String(e)
        }`
      )
    } finally {
      if (mountedRef.current) setSavingRaw(false)
    }
  }, [agent.agent_type, onAffectedSessions, onSaved, rawConfig, t])

  const busy = saving || savingAll

  return (
    <div className="space-y-3 rounded-md border bg-muted/10 p-3">
      <div>
        <label className="text-xs font-medium">{t("configManagement")}</label>
        <p className="mt-1 text-[11px] text-muted-foreground">
          {t("cursor.configDescription")}
        </p>
      </div>

      {/* ---- Authentication method ---- */}
      <div className="space-y-1.5">
        <label className="text-[11px] text-muted-foreground">
          {t("cursor.authMode")}
        </label>
        <Select
          value={mode}
          onValueChange={(value) => setMode(value as CursorAuthMethod)}
        >
          <SelectTrigger aria-label={t("cursor.authMode")} className="w-full">
            <SelectValue />
          </SelectTrigger>
          <SelectContent align="start">
            <SelectItem value="subscription">
              {t("authModeOfficialSubscription")}
            </SelectItem>
            <SelectItem value="custom">{t("cursor.authModeApiKey")}</SelectItem>
          </SelectContent>
        </Select>
        <p className="text-[11px] text-muted-foreground">
          {mode === "subscription"
            ? t("cursor.subscriptionHint")
            : t("cursor.authModeApiKeyHint")}
        </p>
      </div>

      {/* ---- Credential card: shared auth status + method-specific body ---- */}
      <div className="space-y-2 rounded-md border bg-background/60 p-2.5">
        <div className="flex items-center justify-between gap-2">
          <span className="text-[11px] font-medium">
            {t("cursor.authTitle")}
          </span>
          <div className="flex items-center gap-1.5">
            <span
              className={cn(
                "inline-flex h-2 w-2 rounded-full",
                authState === "ok" && "bg-emerald-500",
                authState === "unauthenticated" && "bg-amber-500",
                authState === "missing" && "bg-muted-foreground/40",
                authState === "loading" &&
                  "bg-muted-foreground/40 animate-pulse"
              )}
            />
            <span className="text-[11px] text-muted-foreground">
              {authState === "loading"
                ? t("cursor.authChecking")
                : authState === "missing"
                  ? t("cursor.authNotInstalled")
                  : authState === "ok"
                    ? (auth?.email ?? t("cursor.authLoggedIn"))
                    : t("cursor.authNotLoggedIn")}
            </span>
            {authState === "ok" && auth?.membership ? (
              <span className="rounded bg-emerald-500/10 px-1.5 py-0.5 text-[10px] text-emerald-600 dark:text-emerald-400">
                {auth.membership}
              </span>
            ) : null}
            <Button
              className="h-6 w-6"
              disabled={authLoading}
              onClick={() => void refreshAuth()}
              size="icon"
              type="button"
              variant="ghost"
            >
              <RefreshCw
                className={cn("h-3 w-3", authLoading && "animate-spin")}
              />
            </Button>
          </div>
        </div>

        {/* Subscription: runnable login command when not signed in. */}
        {mode === "subscription" && authState === "unauthenticated" ? (
          <div className="space-y-1.5">
            <p className="text-[11px] text-muted-foreground">
              {t("cursor.loginHint")}
            </p>
            <div className="flex items-center gap-1.5">
              <code className="flex-1 break-all rounded bg-muted px-2 py-1 font-mono text-[11px]">
                {loginCommand}
              </code>
              <Button
                className="h-6 w-6 shrink-0"
                onClick={() => void copyLoginCommand()}
                size="icon"
                type="button"
                variant="ghost"
              >
                {copied ? (
                  <Check className="h-3 w-3 text-emerald-500" />
                ) : (
                  <Copy className="h-3 w-3" />
                )}
              </Button>
            </div>
          </div>
        ) : null}

        {/* Custom mode: explicit Cursor account key and optional endpoint. */}
        {mode === "custom" ? (
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {t("cursor.apiKeyLabel")}
            </label>
            <div className="flex items-center gap-1.5">
              <Input
                className="h-7 flex-1 text-xs"
                onChange={(e) => setApiKey(e.target.value)}
                placeholder={t("cursor.apiKeyPlaceholder")}
                type={showKey ? "text" : "password"}
                value={apiKey}
              />
              <Button
                className="h-7 w-7 shrink-0"
                onClick={() => setShowKey((v) => !v)}
                size="icon"
                type="button"
                variant="ghost"
              >
                {showKey ? (
                  <EyeOff className="h-3.5 w-3.5" />
                ) : (
                  <Eye className="h-3.5 w-3.5" />
                )}
              </Button>
            </div>
            <p className="text-[10px] text-muted-foreground">
              {t("cursor.apiKeyHint")}
            </p>
            <label className="pt-1 text-[11px] text-muted-foreground">
              {t("cursor.baseUrlLabel")}
            </label>
            <Input
              className="h-7 text-xs"
              onChange={(e) => setBaseUrl(e.target.value)}
              placeholder={t("cursor.baseUrlPlaceholder")}
              value={baseUrl}
            />
            <p className="text-[10px] text-muted-foreground">
              {t("cursor.baseUrlHint")}
            </p>
          </div>
        ) : null}

        {auth?.error ? (
          <p className="text-[11px] text-destructive">
            {cursorProbeError(auth.error_code)}
          </p>
        ) : null}

        {/* No model list yet (not signed in / empty): tell the user why the
            picker is hidden, instead of showing an empty default control. */}
        {authState !== "missing" &&
        authState !== "loading" &&
        models.length === 0 ? (
          <p className="text-[10px] text-muted-foreground">
            {t("cursor.modelsNeedAuth")}
          </p>
        ) : null}
      </div>

      {/* ---- Model picker (only when real models were fetched) ---- */}
      {models.length > 0 ? (
        <div className="space-y-2 rounded-md border bg-background/60 p-2.5">
          <div className="flex items-center justify-between gap-2">
            <span className="text-[11px] font-medium">
              {t("cursor.modelTitle")}
            </span>
            <Button
              className="h-6 gap-1 px-2 text-[11px]"
              disabled={modelsLoading}
              onClick={() => void loadModels()}
              size="sm"
              type="button"
              variant="ghost"
            >
              {modelsLoading ? (
                <Loader2 className="h-3 w-3 animate-spin" />
              ) : (
                <RefreshCw className="h-3 w-3" />
              )}
              {t("cursor.loadModels")}
            </Button>
          </div>
          <Combobox
            key={model || UNSET}
            items={modelItems}
            value={selectedModelItem}
            onValueChange={handleSelectModel}
            isItemEqualToValue={(
              a: CursorModelItem | null,
              b: CursorModelItem | null
            ) => (a?.value ?? null) === (b?.value ?? null)}
          >
            <ComboboxInput
              className="h-8 text-xs"
              placeholder={t("cursor.modelPickerPlaceholder")}
              showClear
            />
            <ComboboxContent>
              <ComboboxList>
                {(item: CursorModelItem) => (
                  <ComboboxItem
                    className="text-xs"
                    key={item.value}
                    value={item}
                  >
                    <span className="truncate">{item.label}</span>
                    <span className="ml-auto flex shrink-0 items-center gap-1.5 pl-2 font-mono text-[10px] text-muted-foreground">
                      {item.isDefault ? (
                        <span className="rounded bg-muted px-1 py-0.5 font-sans text-[9px] text-foreground/70">
                          {t("cursor.modelDefaultBadge")}
                        </span>
                      ) : null}
                      {item.value}
                    </span>
                  </ComboboxItem>
                )}
              </ComboboxList>
              <ComboboxEmpty>{t("cursor.modelNoMatch")}</ComboboxEmpty>
            </ComboboxContent>
          </Combobox>
          {modelsError ? (
            <p className="text-[10px] text-muted-foreground">
              {t("cursor.modelsUnavailable")}: {modelsError}
            </p>
          ) : null}
          <p className="text-[10px] text-muted-foreground">
            {t("cursor.modelHint")}
          </p>
        </div>
      ) : null}

      {/* ---- Permissions & sandbox card (both methods) ---- */}
      <div className="space-y-2 rounded-md border bg-background/60 p-2.5">
        <div className="flex items-center gap-1.5">
          <ShieldCheck className="h-3.5 w-3.5 text-muted-foreground" />
          <span className="text-[11px] font-medium">
            {t("cursor.permissionsTitle")}
          </span>
        </div>
        <p className="text-[10px] text-muted-foreground">
          {t("cursor.permissionsDescription")}
        </p>

        <div className="grid gap-2 md:grid-cols-2">
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {t("cursor.permissionModeLabel")}
            </label>
            <Select
              onValueChange={(value) => setForce(value === "force")}
              value={force ? "force" : "default"}
            >
              <SelectTrigger className="h-7 w-full text-xs" size="sm">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem className="text-xs" value="default">
                  {t("cursor.permissionModeDefault")}
                </SelectItem>
                <SelectItem className="text-xs" value="force">
                  {t("cursor.permissionModeForce")}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {t("cursor.sandboxLabel")}
            </label>
            <Select
              onValueChange={(value) =>
                setSandboxMode(value === UNSET ? "" : value)
              }
              value={sandboxMode || UNSET}
            >
              <SelectTrigger className="h-7 w-full text-xs" size="sm">
                <SelectValue />
              </SelectTrigger>
              <SelectContent>
                <SelectItem className="text-xs" value={UNSET}>
                  {t("cursor.optionDefault")}
                </SelectItem>
                <SelectItem className="text-xs" value="enabled">
                  {t("cursor.sandboxEnabled")}
                </SelectItem>
                <SelectItem className="text-xs" value="disabled">
                  {t("cursor.sandboxDisabled")}
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
        </div>
        <p className="text-[10px] text-muted-foreground">
          {t("cursor.permissionModeHint")}
        </p>

        <div className="grid gap-3 md:grid-cols-2">
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {t("cursor.allowRulesLabel")}
            </label>
            <RuleListEditor
              addLabel={t("cursor.addRule")}
              disabled={busy}
              onChange={setAllowRules}
              placeholder="Shell(git)"
              rules={allowRules}
              tone="allow"
            />
          </div>
          <div className="space-y-1">
            <label className="text-[11px] text-muted-foreground">
              {t("cursor.denyRulesLabel")}
            </label>
            <RuleListEditor
              addLabel={t("cursor.addRule")}
              disabled={busy}
              onChange={setDenyRules}
              placeholder="Read(.env*)"
              rules={denyRules}
              tone="deny"
            />
          </div>
        </div>
        <p className="text-[10px] text-muted-foreground">
          {t("cursor.rulesSyntaxHint")}
        </p>
      </div>

      {/* ---- One save for auth method + model + permissions ---- */}
      <div className="flex justify-end">
        <Button
          className="h-7 gap-1.5 px-2.5 text-xs"
          disabled={busy}
          onClick={() => void saveAll()}
          size="sm"
          type="button"
        >
          {busy ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <Save className="h-3.5 w-3.5" />
          )}
          {t("cursor.saveConfig")}
        </Button>
      </div>

      {/* ---- Advanced: raw cli-config.json ---- */}
      <div className="space-y-2">
        <button
          className="flex items-center gap-1 text-[11px] text-muted-foreground hover:text-foreground"
          onClick={() => setAdvancedOpen((v) => !v)}
          type="button"
        >
          {advancedOpen ? (
            <ChevronDown className="h-3 w-3" />
          ) : (
            <ChevronRight className="h-3 w-3" />
          )}
          {t("cursor.advancedToggle")}
        </button>
        {advancedOpen ? (
          <div className="space-y-1.5">
            <p className="text-[10px] text-muted-foreground">
              {t("cursor.advancedHint")}
            </p>
            <Textarea
              className="min-h-40 font-mono text-[11px]"
              onChange={(e) => setRawConfig(e.target.value)}
              spellCheck={false}
              value={rawConfig}
            />
            <div className="flex justify-end">
              <Button
                className="h-7 gap-1.5 px-2.5 text-xs"
                disabled={savingRaw || !rawConfig.trim()}
                onClick={() => void saveRaw()}
                size="sm"
                type="button"
                variant="outline"
              >
                {savingRaw ? (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                ) : (
                  <Save className="h-3.5 w-3.5" />
                )}
                {t("cursor.saveRawConfig")}
              </Button>
            </div>
          </div>
        ) : null}
      </div>
    </div>
  )
}
