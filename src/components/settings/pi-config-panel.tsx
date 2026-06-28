"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  CheckCircle2,
  Cpu,
  Download,
  Eye,
  EyeOff,
  KeyRound,
  Loader2,
  RotateCw,
  Save,
  ShieldCheck,
  TerminalSquare,
  Trash2,
  XCircle,
} from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { Switch } from "@/components/ui/switch"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectSeparator,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  acpInstallPiBinary,
  acpUninstallPiBinary,
  acpUpdatePiConfig,
  acpValidatePiCommand,
  loadPiConfig,
} from "@/lib/api"
import { useAgentInstallStream } from "@/hooks/use-agent-install-stream"
import type { AcpAgentInfo } from "@/lib/types"
import { cn, randomUUID } from "@/lib/utils"

const PI_COMMAND_ENV = "PI_ACP_PI_COMMAND"
const PI_CONFIG_DIR_ENV = "PI_CODING_AGENT_DIR"
const PI_SESSION_DIR_ENV = "PI_CODING_AGENT_SESSION_DIR"
/**
 * Per-agent `env_json` flag gating launch-time workspace-trust seeding. Absent or
 * any value other than `"0"` ⇒ enabled (default on): when codeg connects pi to a
 * folder, the backend marks that folder trusted in pi's `trust.json` so pi loads
 * the project's local config/skills without a separate prompt. `"0"` disables.
 * Read by `seed_pi_workspace_trust` in the Rust launch path.
 */
const PI_TRUST_WORKSPACE_ENV = "PI_ACP_TRUST_WORKSPACE"

/**
 * Reserved env keys the structured pi UI owns. pi-acp reads `PI_ACP_PI_COMMAND`
 * to pick which `pi` binary to spawn, and forwards `PI_CODING_AGENT_DIR` /
 * `PI_CODING_AGENT_SESSION_DIR` to it; `PI_ACP_TRUST_WORKSPACE` is consumed by
 * codeg's own launch path (never the child). These persist through the same
 * per-agent `env_json` path every other env var uses, so the structured UI needs
 * no bespoke storage — the launch pipeline already injects env_json.
 */
export const PI_RESERVED_ENV_KEYS = [
  PI_COMMAND_ENV,
  PI_CONFIG_DIR_ENV,
  PI_SESSION_DIR_ENV,
  PI_TRUST_WORKSPACE_ENV,
] as const

type PiRuntimeMode = "default" | "custom"

const PI_THINKING_LEVELS = [
  "off",
  "minimal",
  "low",
  "medium",
  "high",
  "xhigh",
] as const

/**
 * Curated built-in providers for the enum. pi's authoritative full list lives in
 * its `env-api-keys.ts`; this is the api-key-based subset most users want.
 * Special-auth providers (azure / bedrock / vertex / github-copilot) are omitted
 * on purpose — they don't fit the single-API-key flow; use "Custom" for those.
 */
const PI_BUILTIN_PROVIDERS = [
  "anthropic",
  "openai",
  "google",
  "openrouter",
  "groq",
  "xai",
  "deepseek",
  "cerebras",
  "mistral",
  "together",
  "fireworks",
  "moonshotai",
  "zai",
  "nvidia",
  "minimax",
  "huggingface",
  "vercel-ai-gateway",
]

/** Sentinel Select value that switches the credentials form to custom mode. */
const PI_CUSTOM_SENTINEL = "__custom__"

/** Wire protocols pi accepts for a custom provider in `models.json`. */
const PI_CUSTOM_API_PROTOCOLS = [
  "openai-completions",
  "openai-responses",
  "anthropic-messages",
  "google-generative-ai",
]

type PiValidation = {
  found: boolean
  resolvedPath: string | null
  version: string | null
} | null

/**
 * Build the env map to persist for pi's runtime. `custom` mode writes
 * `PI_ACP_PI_COMMAND` (+ the optional dir overrides); `default` mode clears all
 * three so pi-acp falls back to the `pi` on PATH. Unrelated env keys are
 * preserved untouched, so this never clobbers other per-agent env.
 */
export function buildPiRuntimeEnv(
  prevEnv: Record<string, string>,
  mode: PiRuntimeMode,
  command: string,
  configDir: string,
  sessionDir: string
): Record<string, string> {
  const env: Record<string, string> = { ...prevEnv }
  const cmd = command.trim()
  if (mode === "custom" && cmd) {
    env[PI_COMMAND_ENV] = cmd
    const cfg = configDir.trim()
    if (cfg) env[PI_CONFIG_DIR_ENV] = cfg
    else delete env[PI_CONFIG_DIR_ENV]
    const ses = sessionDir.trim()
    if (ses) env[PI_SESSION_DIR_ENV] = ses
    else delete env[PI_SESSION_DIR_ENV]
  } else {
    delete env[PI_COMMAND_ENV]
    delete env[PI_CONFIG_DIR_ENV]
    delete env[PI_SESSION_DIR_ENV]
  }
  return env
}

/**
 * Dedicated settings panel for pi. Two concerns, two stores:
 *  - Credentials/model — written to pi's native `~/.pi/agent/settings.json`
 *    (`defaultProvider`/`defaultModel`/`defaultThinkingLevel`) and `auth.json`
 *    (the API key) via the `acp_update_pi_config` backend.
 *  - Runtime (bring-your-own-pi) — a visual default↔custom toggle that writes
 *    `PI_ACP_PI_COMMAND` (+ optional config/session dir overrides) into the
 *    per-agent `env_json`, letting users run their own pi build/install.
 */
export function PiConfigPanel({
  agent,
  saving,
  onSaveEnv,
  onSaved,
}: {
  agent: AcpAgentInfo
  saving: boolean
  onSaveEnv: (env: Record<string, string>, enabled: boolean) => Promise<unknown>
  onSaved: () => Promise<void>
}) {
  const t = useTranslations("AcpAgentSettings")

  // --- Credentials (pi's native ~/.pi/agent/{settings,auth,models}.json) ---
  // `selectedProvider` is the Select value: a built-in id, a loaded-but-not-
  // enumerated built-in, or PI_CUSTOM_SENTINEL. In custom mode the effective
  // provider is `customId` (the key written to models.json / auth.json).
  const [selectedProvider, setSelectedProvider] = useState("")
  const [customId, setCustomId] = useState("")
  const [customBaseUrl, setCustomBaseUrl] = useState("")
  const [customApi, setCustomApi] = useState(PI_CUSTOM_API_PROTOCOLS[0])
  const [customProviders, setCustomProviders] = useState<
    { id: string; baseUrl: string; api: string }[]
  >([])
  const [model, setModel] = useState("")
  const [thinkingLevel, setThinkingLevel] = useState("")
  const [apiKey, setApiKey] = useState("")
  const [showKey, setShowKey] = useState(false)
  const [authProviders, setAuthProviders] = useState<string[]>([])
  const [savingCreds, setSavingCreds] = useState(false)
  const [loadingCreds, setLoadingCreds] = useState(true)

  const isCustom = selectedProvider === PI_CUSTOM_SENTINEL
  const effectiveProvider = (isCustom ? customId : selectedProvider).trim()

  useEffect(() => {
    let cancelled = false
    setLoadingCreds(true)
    loadPiConfig()
      .then((cfg) => {
        if (cancelled) return
        setModel(cfg.defaultModel ?? "")
        setThinkingLevel(cfg.defaultThinkingLevel ?? "")
        setAuthProviders(cfg.authProviders ?? [])
        const customs = cfg.customProviders ?? []
        setCustomProviders(customs)
        const dp = cfg.defaultProvider ?? ""
        const matched = customs.find((c) => c.id === dp)
        if (matched) {
          // defaultProvider is a custom/self-hosted provider → custom mode.
          setSelectedProvider(PI_CUSTOM_SENTINEL)
          setCustomId(matched.id)
          setCustomBaseUrl(matched.baseUrl)
          setCustomApi(matched.api || PI_CUSTOM_API_PROTOCOLS[0])
        } else {
          setSelectedProvider(dp)
        }
      })
      .catch((error) => {
        console.error("[Pi] load config failed", error)
      })
      .finally(() => {
        if (!cancelled) setLoadingCreds(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const handleSaveCreds = useCallback(async () => {
    const trimmedModel = model.trim()
    if (!effectiveProvider || !trimmedModel) {
      toast.error(t("pi.providerModelRequired"))
      return
    }
    const trimmedBaseUrl = customBaseUrl.trim()
    if (isCustom && !trimmedBaseUrl) {
      toast.error(t("pi.baseUrlRequired"))
      return
    }
    setSavingCreds(true)
    try {
      await acpUpdatePiConfig({
        provider: effectiveProvider,
        model: trimmedModel,
        thinkingLevel: thinkingLevel || undefined,
        apiKey: apiKey.trim() || undefined,
        customBaseUrl: isCustom ? trimmedBaseUrl : undefined,
        customApi: isCustom ? customApi : undefined,
      })
      if (apiKey.trim()) {
        setApiKey("")
        setAuthProviders((prev) =>
          prev.includes(effectiveProvider)
            ? prev
            : [...prev, effectiveProvider].sort()
        )
      }
      if (isCustom) {
        // Reflect the just-saved custom provider so a reopen rehydrates it.
        setCustomProviders((prev) => {
          const next = prev.filter((c) => c.id !== effectiveProvider)
          next.push({
            id: effectiveProvider,
            baseUrl: trimmedBaseUrl,
            api: customApi,
          })
          next.sort((a, b) => a.id.localeCompare(b.id))
          return next
        })
      }
      await onSaved()
      toast.success(t("toasts.piSaved"))
    } catch (error) {
      console.error("[Pi] save config failed", error)
      toast.error(t("toasts.savePiFailed"))
    } finally {
      setSavingCreds(false)
    }
  }, [
    effectiveProvider,
    isCustom,
    customBaseUrl,
    customApi,
    model,
    thinkingLevel,
    apiKey,
    onSaved,
    t,
  ])

  const providerHasKey =
    effectiveProvider !== "" && authProviders.includes(effectiveProvider)

  // Built-in enum, plus a loaded built-in that isn't in the curated list (so a
  // pre-existing defaultProvider is never dropped from the dropdown).
  const providerOptions =
    selectedProvider &&
    selectedProvider !== PI_CUSTOM_SENTINEL &&
    !PI_BUILTIN_PROVIDERS.includes(selectedProvider)
      ? [...PI_BUILTIN_PROVIDERS, selectedProvider]
      : PI_BUILTIN_PROVIDERS

  const credsIncomplete =
    !effectiveProvider || !model.trim() || (isCustom && !customBaseUrl.trim())

  const handleProviderChange = useCallback(
    (value: string) => {
      setSelectedProvider(value)
      // Switching to custom with nothing typed yet → prefill from an existing
      // custom provider (if any) so a known endpoint need not be re-entered.
      if (
        value === PI_CUSTOM_SENTINEL &&
        !customId.trim() &&
        customProviders[0]
      ) {
        const first = customProviders[0]
        setCustomId(first.id)
        setCustomBaseUrl(first.baseUrl)
        setCustomApi(first.api || PI_CUSTOM_API_PROTOCOLS[0])
      }
    },
    [customId, customProviders]
  )

  // --- pi binary (pi-coding-agent) — the prerequisite pi-acp spawns ---
  // Status reflects the default `pi` on PATH (the global npm package); Install/
  // Uninstall manage that package and stream to the shared install-log block.
  // Surfaced inside the Runtime card's "Default pi" mode (where the global pi is
  // what runs); the bring-your-own-pi override is the "Custom pi" mode.
  const installStream = useAgentInstallStream()
  const {
    status: piInstallStatus,
    logs: piInstallLogs,
    start: startPiInstall,
    reset: resetPiInstall,
  } = installStream
  const installLogEndRef = useRef<HTMLDivElement | null>(null)
  const [piStatus, setPiStatus] = useState<PiValidation>(null)
  const [checkingPi, setCheckingPi] = useState(true)
  const [piOp, setPiOp] = useState<"install" | "uninstall" | null>(null)

  const detectPiBinary = useCallback(async () => {
    setCheckingPi(true)
    try {
      setPiStatus(await acpValidatePiCommand("pi"))
    } catch (error) {
      console.error("[Pi] detect binary failed", error)
      setPiStatus({ found: false, resolvedPath: null, version: null })
    } finally {
      setCheckingPi(false)
    }
  }, [])

  useEffect(() => {
    void detectPiBinary()
  }, [detectPiBinary])

  // Keep the streaming log pinned to its latest line.
  useEffect(() => {
    const container = installLogEndRef.current?.parentElement
    if (container) container.scrollTop = container.scrollHeight
  }, [piInstallLogs])

  // Tear the subscription down on unmount (reset is stable across renders).
  useEffect(() => () => resetPiInstall(), [resetPiInstall])

  const handleInstallPi = useCallback(async () => {
    const taskId = randomUUID()
    setPiOp("install")
    await startPiInstall(taskId)
    try {
      await acpInstallPiBinary(taskId)
      toast.success(t("toasts.piBinaryInstalled"))
      await detectPiBinary()
    } catch (error) {
      console.error("[Pi] install binary failed", error)
      toast.error(t("toasts.piBinaryInstallFailed"))
    } finally {
      setPiOp(null)
    }
  }, [startPiInstall, detectPiBinary, t])

  const handleUninstallPi = useCallback(async () => {
    const taskId = randomUUID()
    setPiOp("uninstall")
    await startPiInstall(taskId)
    try {
      await acpUninstallPiBinary(taskId)
      toast.success(t("toasts.piBinaryUninstalled"))
      await detectPiBinary()
    } catch (error) {
      console.error("[Pi] uninstall binary failed", error)
      toast.error(t("toasts.piBinaryUninstallFailed"))
    } finally {
      setPiOp(null)
    }
  }, [startPiInstall, detectPiBinary, t])

  // --- Runtime (bring-your-own-pi, persisted to env_json reserved keys) ---
  const [mode, setMode] = useState<PiRuntimeMode>(() =>
    (agent.env[PI_COMMAND_ENV] ?? "").trim() ? "custom" : "default"
  )
  const [command, setCommand] = useState(() => agent.env[PI_COMMAND_ENV] ?? "")
  const [configDir, setConfigDir] = useState(
    () => agent.env[PI_CONFIG_DIR_ENV] ?? ""
  )
  const [sessionDir, setSessionDir] = useState(
    () => agent.env[PI_SESSION_DIR_ENV] ?? ""
  )
  const [validating, setValidating] = useState(false)
  const [validation, setValidation] = useState<PiValidation>(null)

  // Workspace trust (default on): seeded into pi's trust.json at launch so pi
  // loads the opened folder's local config/skills without a separate prompt.
  const [trustWorkspace, setTrustWorkspace] = useState(
    () => (agent.env[PI_TRUST_WORKSPACE_ENV] ?? "1") !== "0"
  )
  const [savingTrust, setSavingTrust] = useState(false)

  const handleValidate = useCallback(async () => {
    const cmd = command.trim()
    if (!cmd) return
    setValidating(true)
    setValidation(null)
    try {
      setValidation(await acpValidatePiCommand(cmd))
    } catch (error) {
      console.error("[Pi] validate command failed", error)
      setValidation({ found: false, resolvedPath: null, version: null })
    } finally {
      setValidating(false)
    }
  }, [command])

  const customIncomplete = mode === "custom" && !command.trim()

  const handleSaveRuntime = useCallback(async () => {
    const env = buildPiRuntimeEnv(
      agent.env,
      mode,
      command,
      configDir,
      sessionDir
    )
    try {
      await onSaveEnv(env, agent.enabled)
      toast.success(t("toasts.piRuntimeSaved"))
    } catch (error) {
      console.error("[Pi] save runtime failed", error)
      toast.error(t("toasts.savePiRuntimeFailed"))
    }
  }, [
    agent.env,
    agent.enabled,
    mode,
    command,
    configDir,
    sessionDir,
    onSaveEnv,
    t,
  ])

  // Self-persisting toggle: write the flag straight to env_json on change. Default
  // on ⇒ omit the key when enabling (absence = default), write "0" when disabling.
  const handleToggleTrust = useCallback(
    async (next: boolean) => {
      setTrustWorkspace(next)
      setSavingTrust(true)
      const env = { ...agent.env }
      if (next) delete env[PI_TRUST_WORKSPACE_ENV]
      else env[PI_TRUST_WORKSPACE_ENV] = "0"
      try {
        await onSaveEnv(env, agent.enabled)
      } catch (error) {
        console.error("[Pi] save workspace trust failed", error)
        setTrustWorkspace(!next)
        toast.error(t("toasts.savePiTrustFailed"))
      } finally {
        setSavingTrust(false)
      }
    },
    [agent.env, agent.enabled, onSaveEnv, t]
  )

  return (
    <div className="space-y-4">
      {/* Runtime — which pi binary pi-acp spawns. "Default pi" manages the
          global pi on PATH (install XOR uninstall, never both); "Custom pi"
          points at your own build. */}
      <div className="space-y-3 rounded-md border bg-muted/10 p-3">
        <div>
          <label className="flex items-center gap-1.5 text-xs font-medium">
            <Cpu className="h-3.5 w-3.5 text-muted-foreground" />
            {t("pi.runtimeTitle")}
          </label>
          <p className="mt-1 text-[11px] text-muted-foreground">
            {t("pi.runtimeDescription")}
          </p>
        </div>

        <RadioGroup
          value={mode}
          onValueChange={(value) => setMode(value as PiRuntimeMode)}
          className="grid-cols-2"
        >
          <label
            htmlFor="pi-mode-default"
            className={cn(
              "flex cursor-pointer items-start gap-2 rounded-md border p-2.5 text-[11px]",
              mode === "default"
                ? "border-primary bg-primary/5"
                : "border-input"
            )}
          >
            <RadioGroupItem
              value="default"
              id="pi-mode-default"
              className="mt-0.5"
            />
            <span>
              <span className="block font-medium text-foreground">
                {t("pi.modeDefault")}
              </span>
              <span className="mt-0.5 block text-muted-foreground">
                {t("pi.modeDefaultHint")}
              </span>
            </span>
          </label>
          <label
            htmlFor="pi-mode-custom"
            className={cn(
              "flex cursor-pointer items-start gap-2 rounded-md border p-2.5 text-[11px]",
              mode === "custom" ? "border-primary bg-primary/5" : "border-input"
            )}
          >
            <RadioGroupItem
              value="custom"
              id="pi-mode-custom"
              className="mt-0.5"
            />
            <span>
              <span className="block font-medium text-foreground">
                {t("pi.modeCustom")}
              </span>
              <span className="mt-0.5 block text-muted-foreground">
                {t("pi.modeCustomHint")}
              </span>
            </span>
          </label>
        </RadioGroup>

        {/* Default pi → status of the global `pi`, with a single contextual
            action: Install when missing, Uninstall when present (never both). */}
        {mode === "default" && (
          <div className="space-y-2.5 rounded-md border border-dashed p-2.5">
            <div className="flex items-start justify-between gap-2">
              <div className="min-w-0 flex-1 text-[11px]">
                {checkingPi ? (
                  <span className="flex items-center gap-1.5 text-muted-foreground">
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    {t("pi.binaryChecking")}
                  </span>
                ) : piStatus?.found ? (
                  <span className="flex items-start gap-1.5 text-emerald-600">
                    <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                    <span className="min-w-0">
                      <span className="font-medium">
                        {t("pi.binaryInstalled")}
                      </span>
                      {piStatus.version ? ` · ${piStatus.version}` : ""}
                      {piStatus.resolvedPath ? (
                        <span className="mt-0.5 block break-all text-muted-foreground">
                          {piStatus.resolvedPath}
                        </span>
                      ) : null}
                    </span>
                  </span>
                ) : (
                  <span className="flex items-center gap-1.5 text-muted-foreground">
                    <XCircle className="h-3.5 w-3.5 shrink-0" />
                    {t("pi.binaryMissing")}
                  </span>
                )}
              </div>
              <div className="flex shrink-0 items-center gap-1.5">
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={detectPiBinary}
                  disabled={checkingPi || piOp !== null}
                  title={t("pi.recheck")}
                  className="h-7 px-2"
                >
                  <RotateCw
                    className={cn("h-3.5 w-3.5", checkingPi && "animate-spin")}
                  />
                </Button>
                {!checkingPi &&
                  (piStatus?.found ? (
                    <Button
                      type="button"
                      variant="outline"
                      size="sm"
                      onClick={handleUninstallPi}
                      disabled={piOp !== null}
                      className="gap-1.5"
                    >
                      {piOp === "uninstall" ? (
                        <>
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                          {t("actions.uninstalling")}
                        </>
                      ) : (
                        <>
                          <Trash2 className="h-3.5 w-3.5" />
                          {t("actions.uninstall")}
                        </>
                      )}
                    </Button>
                  ) : (
                    <Button
                      type="button"
                      size="sm"
                      onClick={handleInstallPi}
                      disabled={piOp !== null}
                      className="gap-1.5"
                    >
                      {piOp === "install" ? (
                        <>
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                          {t("pi.installing")}
                        </>
                      ) : (
                        <>
                          <Download className="h-3.5 w-3.5" />
                          {t("pi.installBinary")}
                        </>
                      )}
                    </Button>
                  ))}
              </div>
            </div>

            {piInstallStatus !== "idle" && (
              <div className="max-h-[200px] overflow-y-auto rounded-md border bg-muted/50 p-3 font-mono text-[11px] leading-relaxed text-muted-foreground">
                {piInstallLogs.map((line, i) => (
                  <div
                    key={i}
                    className={
                      line.startsWith("ERROR:") ? "text-destructive" : ""
                    }
                  >
                    {line}
                  </div>
                ))}
                <div ref={installLogEndRef} />
              </div>
            )}
          </div>
        )}

        {/* Custom pi → bring your own build / wrapper. */}
        {mode === "custom" && (
          <>
            <div className="space-y-1.5">
              <label className="text-[11px] text-muted-foreground">
                {t("pi.commandLabel")}
              </label>
              <div className="flex items-center gap-2">
                <Input
                  value={command}
                  onChange={(event) => {
                    setCommand(event.target.value)
                    setValidation(null)
                  }}
                  placeholder="/path/to/pi · pi · ./pi-test.sh"
                  spellCheck={false}
                />
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={handleValidate}
                  disabled={validating || !command.trim()}
                  className="gap-1.5 whitespace-nowrap"
                >
                  {validating ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  ) : (
                    <TerminalSquare className="h-3.5 w-3.5" />
                  )}
                  {t("pi.validate")}
                </Button>
              </div>
              {validation && (
                <p
                  className={cn(
                    "flex items-start gap-1.5 text-[11px]",
                    validation.found ? "text-emerald-600" : "text-destructive"
                  )}
                >
                  {validation.found ? (
                    <>
                      <CheckCircle2 className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                      <span className="break-all">
                        {validation.resolvedPath}
                        {validation.version ? ` (${validation.version})` : ""}
                      </span>
                    </>
                  ) : (
                    <>
                      <XCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />
                      {t("pi.commandNotFound")}
                    </>
                  )}
                </p>
              )}
              <p className="text-[11px] text-muted-foreground">
                {t("pi.commandHint")}
              </p>
            </div>

            <details className="rounded-md border border-dashed">
              <summary className="cursor-pointer list-none px-2.5 py-1.5 text-[11px] font-medium text-muted-foreground">
                {t("pi.advanced")}
              </summary>
              <div className="space-y-2.5 px-2.5 pb-2.5">
                <div className="space-y-1.5">
                  <label className="text-[11px] text-muted-foreground">
                    {t("pi.configDirLabel")}
                  </label>
                  <Input
                    value={configDir}
                    onChange={(event) => setConfigDir(event.target.value)}
                    placeholder="~/.pi/agent"
                    spellCheck={false}
                  />
                </div>
                <div className="space-y-1.5">
                  <label className="text-[11px] text-muted-foreground">
                    {t("pi.sessionDirLabel")}
                  </label>
                  <Input
                    value={sessionDir}
                    onChange={(event) => setSessionDir(event.target.value)}
                    placeholder="~/.pi/agent/sessions"
                    spellCheck={false}
                  />
                </div>
                <p className="text-[11px] text-muted-foreground">
                  {t("pi.flagsHint")}
                </p>
              </div>
            </details>
          </>
        )}

        <div className="flex items-center justify-between gap-2">
          <span className="text-[11px] text-muted-foreground">
            {customIncomplete ? t("pi.customIncomplete") : ""}
          </span>
          <Button
            type="button"
            size="sm"
            onClick={handleSaveRuntime}
            disabled={saving || customIncomplete}
            className="gap-1.5"
          >
            {saving ? (
              <>
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                {t("actions.saving")}
              </>
            ) : (
              <>
                <Save className="h-3.5 w-3.5" />
                {t("pi.saveRuntime")}
              </>
            )}
          </Button>
        </div>
      </div>

      {/* Credentials / model — pi's native settings.json / auth.json */}
      <div className="space-y-3 rounded-md border bg-muted/10 p-3">
        <div>
          <label className="flex items-center gap-1.5 text-xs font-medium">
            <KeyRound className="h-3.5 w-3.5 text-muted-foreground" />
            {t("pi.configManagement")}
          </label>
          <p className="mt-1 text-[11px] text-muted-foreground">
            {t("pi.configDescription")}
          </p>
        </div>

        <div className="space-y-1.5">
          <label className="text-[11px] text-muted-foreground">
            {t("pi.providerLabel")}
          </label>
          <Select
            value={selectedProvider}
            onValueChange={handleProviderChange}
            disabled={savingCreds || loadingCreds}
          >
            <SelectTrigger className="w-full">
              <SelectValue placeholder={t("pi.providerPlaceholder")} />
            </SelectTrigger>
            <SelectContent align="start">
              {providerOptions.map((p) => (
                <SelectItem key={p} value={p}>
                  {p}
                </SelectItem>
              ))}
              <SelectSeparator />
              <SelectItem value={PI_CUSTOM_SENTINEL}>
                {t("pi.customProvider")}
              </SelectItem>
            </SelectContent>
          </Select>
        </div>

        {isCustom && (
          <div className="space-y-2.5 rounded-md border border-dashed p-2.5">
            <div className="grid grid-cols-2 gap-2">
              <div className="space-y-1.5">
                <label className="text-[11px] text-muted-foreground">
                  {t("pi.providerIdLabel")}
                </label>
                <Input
                  value={customId}
                  onChange={(event) => setCustomId(event.target.value)}
                  placeholder="my-provider"
                  spellCheck={false}
                  disabled={savingCreds || loadingCreds}
                />
              </div>
              <div className="space-y-1.5">
                <label className="text-[11px] text-muted-foreground">
                  {t("pi.apiProtocolLabel")}
                </label>
                <Select
                  value={customApi}
                  onValueChange={setCustomApi}
                  disabled={savingCreds || loadingCreds}
                >
                  <SelectTrigger className="w-full">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent align="start">
                    {PI_CUSTOM_API_PROTOCOLS.map((api) => (
                      <SelectItem key={api} value={api}>
                        {api}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>
              </div>
            </div>
            <div className="space-y-1.5">
              <label className="text-[11px] text-muted-foreground">
                {t("pi.baseUrlLabel")}
              </label>
              <Input
                value={customBaseUrl}
                onChange={(event) => setCustomBaseUrl(event.target.value)}
                placeholder="https://api.example.com/v1"
                spellCheck={false}
                disabled={savingCreds || loadingCreds}
              />
            </div>
            <p className="text-[11px] text-muted-foreground">
              {t("pi.customProviderHint")}
            </p>
          </div>
        )}

        <div className="space-y-1.5">
          <label className="text-[11px] text-muted-foreground">
            {t("pi.modelLabel")}
          </label>
          <Input
            value={model}
            onChange={(event) => setModel(event.target.value)}
            placeholder="claude-sonnet-4-20250514"
            spellCheck={false}
            disabled={savingCreds || loadingCreds}
          />
        </div>

        <div className="space-y-1.5">
          <label className="text-[11px] text-muted-foreground">
            {t("pi.thinkingLabel")}
          </label>
          <Select
            value={thinkingLevel || "off"}
            onValueChange={(value) => setThinkingLevel(value)}
            disabled={savingCreds || loadingCreds}
          >
            <SelectTrigger className="w-full">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="start">
              {PI_THINKING_LEVELS.map((level) => (
                <SelectItem key={level} value={level}>
                  {t(`pi.thinking.${level}`)}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        </div>

        <div className="space-y-1.5">
          <label className="text-[11px] text-muted-foreground">
            {t("pi.apiKeyLabel")}
          </label>
          <div className="flex items-center gap-2">
            <Input
              type={showKey ? "text" : "password"}
              value={apiKey}
              onChange={(event) => setApiKey(event.target.value)}
              placeholder={
                providerHasKey ? t("pi.apiKeySetPlaceholder") : "sk-..."
              }
              disabled={savingCreds || loadingCreds}
            />
            <Button
              type="button"
              variant="outline"
              size="sm"
              onClick={() => setShowKey((prev) => !prev)}
              title={
                showKey ? t("actions.hideApiKey") : t("actions.showApiKey")
              }
            >
              {showKey ? (
                <EyeOff className="h-3.5 w-3.5" />
              ) : (
                <Eye className="h-3.5 w-3.5" />
              )}
            </Button>
          </div>
          <p className="text-[11px] text-muted-foreground">
            {t("pi.apiKeyHint")}
          </p>
        </div>

        <div className="flex justify-end">
          <Button
            type="button"
            size="sm"
            onClick={handleSaveCreds}
            disabled={savingCreds || loadingCreds || credsIncomplete}
            className="gap-1.5"
          >
            {savingCreds ? (
              <>
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                {t("actions.saving")}
              </>
            ) : (
              <>
                <Save className="h-3.5 w-3.5" />
                {t("pi.saveConfig")}
              </>
            )}
          </Button>
        </div>
      </div>

      {/* Workspace trust — auto-trust the folder codeg launches pi into */}
      <div className="space-y-2 rounded-md border bg-muted/10 p-3">
        <div className="flex items-start justify-between gap-3">
          <div className="min-w-0">
            <label
              htmlFor="pi-trust-workspace"
              className="flex items-center gap-1.5 text-xs font-medium"
            >
              <ShieldCheck className="h-3.5 w-3.5 text-muted-foreground" />
              {t("pi.trustTitle")}
            </label>
            <p className="mt-1 text-[11px] text-muted-foreground">
              {t("pi.trustDescription")}
            </p>
          </div>
          <Switch
            id="pi-trust-workspace"
            checked={trustWorkspace}
            onCheckedChange={handleToggleTrust}
            disabled={savingTrust}
          />
        </div>
        <p className="text-[11px] text-muted-foreground">{t("pi.trustHint")}</p>
      </div>
    </div>
  )
}
