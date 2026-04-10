"use client"

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent,
  type ReactNode,
} from "react"
import { Reorder, useDragControls } from "motion/react"
import { useLocale, useTranslations } from "next-intl"
import { useSearchParams } from "next/navigation"
import {
  AlertCircle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Download,
  Eye,
  EyeOff,
  GripVertical,
  Loader2,
  Minus,
  RefreshCw,
  Save,
  Trash2,
  Wrench,
} from "lucide-react"
import { openUrl } from "@/lib/platform"
import { toast } from "sonner"
import { AgentIcon } from "@/components/agent-icon"
import {
  AlertDialog,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Collapsible, CollapsibleContent } from "@/components/ui/collapsible"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Textarea } from "@/components/ui/textarea"
import {
  Combobox,
  ComboboxContent,
  ComboboxEmpty,
  ComboboxGroup,
  ComboboxInput,
  ComboboxItem,
  ComboboxLabel,
  ComboboxList,
} from "@/components/ui/combobox"
import { cn } from "@/lib/utils"
import {
  acpClearBinaryCache,
  acpDetectAgentLocalVersion,
  acpDownloadAgentBinary,
  acpListAgents,
  acpPreflight,
  acpPrepareNpxAgent,
  acpReorderAgents,
  acpUninstallAgent,
  acpUpdateAgentConfig,
  acpUpdateAgentEnv,
  listModelProviders,
} from "@/lib/api"
import type {
  AcpAgentInfo,
  AgentType,
  CheckStatus,
  FixAction,
  ModelProviderInfo,
  PreflightResult,
} from "@/lib/types"

interface AgentCheckState {
  result?: PreflightResult
  error?: string
}

const CLAUDE_AUTH_MODES = [
  "official_subscription",
  "custom",
  "model_provider",
] as const
type ClaudeAuthMode = (typeof CLAUDE_AUTH_MODES)[number]

interface AgentDraft {
  enabled: boolean
  envText: string
  configText: string
  apiBaseUrl: string
  apiKey: string
  model: string
  claudeAuthMode: ClaudeAuthMode
  modelProviderId: number | null
  geminiAuthMode: GeminiAuthMode
  geminiApiKey: string
  googleApiKey: string
  googleCloudProject: string
  googleCloudLocation: string
  googleApplicationCredentials: string
  codexAuthMode: CodexAuthMode
  codexModelProvider: string
  codexProviderOptions: string[]
  codexReasoningEffort: CodexReasoningEffort
  codexSupportsWebsockets: boolean
  claudeMainModel: string
  claudeReasoningModel: string
  claudeDefaultHaikuModel: string
  claudeDefaultSonnetModel: string
  claudeDefaultOpusModel: string
  codexAuthJsonText: string
  codexConfigTomlText: string
  openCodeAuthJsonText: string
  openClawGatewayUrl: string
  openClawGatewayToken: string
  openClawSessionKey: string
  clineProvider: ClineProvider
  clineApiKey: string
  clineModel: string
  clineBaseUrl: string
}

type RunningActionKind =
  | "download_binary"
  | "upgrade_binary"
  | "install_npx"
  | "upgrade_npx"
  | "uninstall_binary"
  | "uninstall_npx"
  | "redownload_binary"

type UiFixAction =
  | FixAction
  | {
      label: string
      kind:
        | "download_binary"
        | "upgrade_binary"
        | "install_npx"
        | "upgrade_npx"
        | "uninstall_binary"
        | "uninstall_npx"
      payload: string
    }

interface UiCheckItem {
  check_id: string
  label: string
  status: CheckStatus
  message: string
  fixes: UiFixAction[]
}

type AcpTranslator = (
  key: string,
  values?: Record<string, string | number>
) => string

let acpTranslator: AcpTranslator | null = null

function acpText(
  key: string,
  fallback: string,
  values?: Record<string, string | number>
): string {
  if (!acpTranslator) return fallback
  return acpTranslator(key, values)
}

function statusTone(status: CheckStatus): string {
  if (status === "pass") return "text-green-500"
  if (status === "warn") return "text-yellow-500"
  return "text-red-500"
}

function summarizeChecks(checks: UiCheckItem[]): CheckStatus | "unchecked" {
  if (checks.length === 0) return "unchecked"
  if (checks.some((check) => check.status === "fail")) return "fail"
  if (checks.some((check) => check.status === "warn")) return "warn"
  return "pass"
}

function envMapToText(env: Record<string, string>): string {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join("\n")
}

function parseEnvText(envText: string): Record<string, string> {
  const map: Record<string, string> = {}
  for (const rawLine of envText.split(/\r?\n/)) {
    const line = rawLine.trim()
    if (!line || line.startsWith("#")) continue
    const idx = line.indexOf("=")
    if (idx <= 0) continue
    const key = line.slice(0, idx).trim()
    const value = line.slice(idx + 1).trim()
    if (!key) continue
    map[key] = value
  }
  return map
}

function patchEnvText(
  envText: string,
  patch: Record<string, string | undefined>
): string {
  const envMap = parseEnvText(envText)
  for (const [key, value] of Object.entries(patch)) {
    const trimmed = value?.trim() ?? ""
    if (!trimmed) {
      delete envMap[key]
    } else {
      envMap[key] = trimmed
    }
  }
  return envMapToText(envMap)
}

interface ImportantEnvKeys {
  apiBaseUrl: string[]
  apiKey: string[]
  model: string[]
}

const CLAUDE_MODEL_ENV_KEYS = {
  claudeMainModel: "ANTHROPIC_MODEL",
  claudeReasoningModel: "ANTHROPIC_REASONING_MODEL",
  claudeDefaultHaikuModel: "ANTHROPIC_DEFAULT_HAIKU_MODEL",
  claudeDefaultSonnetModel: "ANTHROPIC_DEFAULT_SONNET_MODEL",
  claudeDefaultOpusModel: "ANTHROPIC_DEFAULT_OPUS_MODEL",
} as const

const GEMINI_AUTH_MODES = [
  "custom",
  "login_google",
  "gemini_api_key",
  "vertex_adc",
  "vertex_service_account",
  "vertex_api_key",
  "model_provider",
] as const

type GeminiAuthMode = (typeof GEMINI_AUTH_MODES)[number]

const GEMINI_ENV_KEYS = {
  baseUrl: "GOOGLE_GEMINI_BASE_URL",
  legacyBaseUrl: "GEMINI_BASE_URL",
  geminiApiKey: "GEMINI_API_KEY",
  legacyGeminiApiKey: "GOOGLE_GEMINI_API_KEY",
  googleApiKey: "GOOGLE_API_KEY",
  cloudProject: "GOOGLE_CLOUD_PROJECT",
  cloudProjectLegacy: "GOOGLE_CLOUD_PROJECT_ID",
  cloudLocation: "GOOGLE_CLOUD_LOCATION",
  applicationCredentials: "GOOGLE_APPLICATION_CREDENTIALS",
  model: "GEMINI_MODEL",
} as const

const OPENCLAW_ENV_KEYS = {
  gatewayUrl: "OPENCLAW_GATEWAY_URL",
  gatewayToken: "OPENCLAW_GATEWAY_TOKEN",
  sessionKey: "OPENCLAW_SESSION_KEY",
} as const

const CLINE_PROVIDERS = [
  { value: "anthropic", label: "Anthropic" },
  { value: "openai-native", label: "OpenAI" },
  { value: "openai", label: "OpenAI Compatible" },
  { value: "openrouter", label: "OpenRouter" },
  { value: "gemini", label: "Gemini" },
  { value: "deepseek", label: "DeepSeek" },
  { value: "bedrock", label: "AWS Bedrock" },
  { value: "vertex", label: "GCP Vertex" },
  { value: "ollama", label: "Ollama" },
] as const

type ClineProvider = (typeof CLINE_PROVIDERS)[number]["value"]

type ClaudeModelKey = keyof typeof CLAUDE_MODEL_ENV_KEYS
type ImportantConfigKey = "apiBaseUrl" | "apiKey" | "model" | ClaudeModelKey
type ImportantDraftPatch = Partial<Pick<AgentDraft, ImportantConfigKey>>

interface ConfigParseResult {
  config: Record<string, unknown>
  error: string | null
}

function importantEnvKeysByAgent(agentType: AgentType): ImportantEnvKeys {
  if (agentType === "claude_code") {
    return {
      apiBaseUrl: ["ANTHROPIC_BASE_URL", "OPENAI_BASE_URL", "API_BASE_URL"],
      apiKey: ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "OPENAI_API_KEY"],
      model: ["ANTHROPIC_MODEL", "OPENAI_MODEL", "MODEL"],
    }
  }
  if (agentType === "gemini") {
    return {
      apiBaseUrl: ["GOOGLE_GEMINI_BASE_URL", "GEMINI_BASE_URL", "API_BASE_URL"],
      apiKey: [
        GEMINI_ENV_KEYS.geminiApiKey,
        GEMINI_ENV_KEYS.googleApiKey,
        GEMINI_ENV_KEYS.legacyGeminiApiKey,
        "API_KEY",
      ],
      model: ["GEMINI_MODEL", "MODEL"],
    }
  }
  return {
    apiBaseUrl: ["OPENAI_BASE_URL", "API_BASE_URL"],
    apiKey: ["OPENAI_API_KEY", "API_KEY"],
    model: ["OPENAI_MODEL", "MODEL"],
  }
}

function parseConfigJsonText(configText: string): ConfigParseResult {
  const trimmed = configText.trim()
  if (!trimmed) return { config: {}, error: null }

  try {
    const parsed = JSON.parse(trimmed) as unknown
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {
        config: {},
        error: acpText(
          "errors.nativeJsonMustBeObject",
          "Native JSON config must be an object"
        ),
      }
    }
    return { config: parsed as Record<string, unknown>, error: null }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    return {
      config: {},
      error: acpText(
        "errors.nativeJsonInvalid",
        "Native JSON config format error: {message}",
        { message }
      ),
    }
  }
}

function asObjectRecord(value: unknown): Record<string, unknown> | null {
  if (!value || typeof value !== "object" || Array.isArray(value)) return null
  return value as Record<string, unknown>
}

function parseOpenCodeAuthJsonText(authJsonText: string): {
  authObject: Record<string, unknown> | null
  error: string | null
} {
  const trimmed = authJsonText.trim()
  if (!trimmed) return { authObject: {}, error: null }
  try {
    const parsed = JSON.parse(trimmed) as unknown
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {
        authObject: null,
        error: acpText(
          "errors.openCodeAuthMustBeObject",
          "OpenCode auth.json must be a JSON object"
        ),
      }
    }
    return { authObject: parsed as Record<string, unknown>, error: null }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    return {
      authObject: null,
      error: acpText(
        "errors.openCodeAuthInvalid",
        "OpenCode auth.json format error: {message}",
        { message }
      ),
    }
  }
}

function patchOpenCodeAuthJsonText(
  authJsonText: string,
  mutator: (authObject: Record<string, unknown>) => void
): { authJsonText: string; recoveredFromInvalid: boolean } {
  const parsed = parseOpenCodeAuthJsonText(authJsonText)
  const authObject = parsed.error
    ? {}
    : (JSON.parse(JSON.stringify(parsed.authObject ?? {})) as Record<
        string,
        unknown
      >)
  mutator(authObject)
  return {
    authJsonText:
      Object.keys(authObject).length === 0
        ? ""
        : JSON.stringify(authObject, null, 2),
    recoveredFromInvalid: Boolean(parsed.error),
  }
}

function envFromConfig(
  config: Record<string, unknown>
): Record<string, string> {
  const raw = config.env
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    return {}
  }

  const map: Record<string, string> = {}
  for (const [key, value] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof value !== "string") continue
    const trimmedKey = key.trim()
    const trimmedValue = value.trim()
    if (!trimmedKey || !trimmedValue) continue
    map[trimmedKey] = trimmedValue
  }
  return map
}

function pickFirstString(
  source: Record<string, unknown>,
  keys: string[]
): string | null {
  for (const key of keys) {
    const value = source[key]
    if (typeof value !== "string") continue
    const trimmed = value.trim()
    if (trimmed) return trimmed
  }
  return null
}

function findEnvValue(env: Record<string, string>, keys: string[]): string {
  for (const key of keys) {
    const value = env[key]
    if (!value) continue
    const trimmed = value.trim()
    if (trimmed) return trimmed
  }
  return ""
}

function extractImportantConfigValues(
  agentType: AgentType,
  env: Record<string, string>,
  configText: string
): {
  apiBaseUrl: string
  apiKey: string
  model: string
  claudeMainModel: string
  claudeReasoningModel: string
  claudeDefaultHaikuModel: string
  claudeDefaultSonnetModel: string
  claudeDefaultOpusModel: string
  configError: string | null
} {
  const parseResult = parseConfigJsonText(configText)
  const config = parseResult.config
  const keys = importantEnvKeysByAgent(agentType)

  const configEnv = envFromConfig(config)
  const mergedEnv = { ...env, ...configEnv }

  const apiBaseUrl =
    pickFirstString(config, ["apiBaseUrl", "api_base_url"]) ??
    findEnvValue(mergedEnv, keys.apiBaseUrl)
  const apiKey =
    pickFirstString(config, ["apiKey", "api_key"]) ??
    findEnvValue(mergedEnv, keys.apiKey)
  const model =
    pickFirstString(config, ["model", "model_name"]) ??
    findEnvValue(mergedEnv, keys.model)
  const claudeMainModel = findEnvValue(mergedEnv, [
    CLAUDE_MODEL_ENV_KEYS.claudeMainModel,
  ])
  const claudeReasoningModel = findEnvValue(mergedEnv, [
    CLAUDE_MODEL_ENV_KEYS.claudeReasoningModel,
  ])
  const claudeDefaultHaikuModel = findEnvValue(mergedEnv, [
    CLAUDE_MODEL_ENV_KEYS.claudeDefaultHaikuModel,
  ])
  const claudeDefaultSonnetModel = findEnvValue(mergedEnv, [
    CLAUDE_MODEL_ENV_KEYS.claudeDefaultSonnetModel,
  ])
  const claudeDefaultOpusModel = findEnvValue(mergedEnv, [
    CLAUDE_MODEL_ENV_KEYS.claudeDefaultOpusModel,
  ])

  return {
    apiBaseUrl: apiBaseUrl ?? "",
    apiKey: apiKey ?? "",
    model: model ?? "",
    claudeMainModel: agentType === "claude_code" ? (claudeMainModel ?? "") : "",
    claudeReasoningModel:
      agentType === "claude_code" ? claudeReasoningModel : "",
    claudeDefaultHaikuModel:
      agentType === "claude_code" ? claudeDefaultHaikuModel : "",
    claudeDefaultSonnetModel:
      agentType === "claude_code" ? claudeDefaultSonnetModel : "",
    claudeDefaultOpusModel:
      agentType === "claude_code" ? claudeDefaultOpusModel : "",
    configError: parseResult.error,
  }
}

interface GeminiImportantValues {
  authMode: GeminiAuthMode
  apiBaseUrl: string
  geminiApiKey: string
  googleApiKey: string
  googleCloudProject: string
  googleCloudLocation: string
  googleApplicationCredentials: string
  model: string
}

function inferGeminiAuthMode(values: {
  apiBaseUrl: string
  geminiApiKey: string
  googleApiKey: string
  googleCloudProject: string
  googleCloudLocation: string
  googleApplicationCredentials: string
}): GeminiAuthMode {
  if (values.apiBaseUrl.trim()) return "custom"
  if (values.geminiApiKey.trim()) return "gemini_api_key"
  if (values.googleApiKey.trim()) return "vertex_api_key"
  if (values.googleApplicationCredentials.trim())
    return "vertex_service_account"
  if (values.googleCloudProject.trim() || values.googleCloudLocation.trim()) {
    return "vertex_adc"
  }
  return "login_google"
}

function extractGeminiImportantValues(
  env: Record<string, string>,
  configText: string
): GeminiImportantValues {
  const parseResult = parseConfigJsonText(configText)
  const config = parseResult.config
  const configEnv = envFromConfig(config)
  const mergedEnv = { ...env, ...configEnv }

  const apiBaseUrl = findEnvValue(mergedEnv, [
    GEMINI_ENV_KEYS.baseUrl,
    GEMINI_ENV_KEYS.legacyBaseUrl,
    "API_BASE_URL",
  ])
  const geminiApiKey = findEnvValue(mergedEnv, [
    GEMINI_ENV_KEYS.geminiApiKey,
    GEMINI_ENV_KEYS.legacyGeminiApiKey,
  ])
  const googleApiKey = findEnvValue(mergedEnv, [GEMINI_ENV_KEYS.googleApiKey])
  const googleCloudProject = findEnvValue(mergedEnv, [
    GEMINI_ENV_KEYS.cloudProject,
    GEMINI_ENV_KEYS.cloudProjectLegacy,
  ])
  const googleCloudLocation = findEnvValue(mergedEnv, [
    GEMINI_ENV_KEYS.cloudLocation,
  ])
  const googleApplicationCredentials = findEnvValue(mergedEnv, [
    GEMINI_ENV_KEYS.applicationCredentials,
  ])
  const model = findEnvValue(mergedEnv, [GEMINI_ENV_KEYS.model, "MODEL"])

  return {
    authMode: inferGeminiAuthMode({
      apiBaseUrl,
      geminiApiKey,
      googleApiKey,
      googleCloudProject,
      googleCloudLocation,
      googleApplicationCredentials,
    }),
    apiBaseUrl,
    geminiApiKey,
    googleApiKey,
    googleCloudProject,
    googleCloudLocation,
    googleApplicationCredentials,
    model: model ?? "",
  }
}

interface OpenClawImportantValues {
  gatewayUrl: string
  gatewayToken: string
  sessionKey: string
}

interface ClineImportantValues {
  provider: ClineProvider
  apiKey: string
  model: string
  baseUrl: string
}

function extractClineImportantValues(configText: string): ClineImportantValues {
  const parseResult = parseConfigJsonText(configText)
  const config = parseResult.config
  return {
    provider: (typeof config.apiProvider === "string" && config.apiProvider
      ? config.apiProvider
      : "anthropic") as ClineProvider,
    apiKey: typeof config.apiKey === "string" ? config.apiKey : "",
    model: typeof config.model === "string" ? config.model : "",
    baseUrl: typeof config.apiBaseUrl === "string" ? config.apiBaseUrl : "",
  }
}

function extractOpenClawImportantValues(
  env: Record<string, string>,
  configText: string
): OpenClawImportantValues {
  const parseResult = parseConfigJsonText(configText)
  const config = parseResult.config
  const configEnv = envFromConfig(config)
  const mergedEnv = { ...env, ...configEnv }

  return {
    gatewayUrl: findEnvValue(mergedEnv, [OPENCLAW_ENV_KEYS.gatewayUrl]),
    gatewayToken: findEnvValue(mergedEnv, [OPENCLAW_ENV_KEYS.gatewayToken]),
    sessionKey: findEnvValue(mergedEnv, [OPENCLAW_ENV_KEYS.sessionKey]),
  }
}

function patchGeminiConfigText(
  configText: string,
  patch: {
    apiBaseUrl?: string
    model?: string
    geminiApiKey?: string
    googleApiKey?: string
    googleCloudProject?: string
    googleCloudLocation?: string
    googleApplicationCredentials?: string
  }
): {
  configText: string
  recoveredFromInvalid: boolean
} {
  const parseResult = parseConfigJsonText(configText)
  const config = parseResult.error ? {} : { ...parseResult.config }
  const env =
    typeof config.env === "object" && config.env && !Array.isArray(config.env)
      ? { ...(config.env as Record<string, unknown>) }
      : {}

  const assignOrRemoveEnv = (key: string, value: string | undefined) => {
    if (typeof value !== "string") return
    const trimmed = value.trim()
    if (!trimmed) {
      delete env[key]
      return
    }
    env[key] = trimmed
  }

  if (typeof patch.model === "string") {
    delete config.model
    delete config.model_name
    assignOrRemoveEnv(GEMINI_ENV_KEYS.model, patch.model)
  }
  assignOrRemoveEnv(GEMINI_ENV_KEYS.baseUrl, patch.apiBaseUrl)
  if (typeof patch.apiBaseUrl === "string") {
    assignOrRemoveEnv(GEMINI_ENV_KEYS.legacyBaseUrl, "")
  }
  assignOrRemoveEnv(GEMINI_ENV_KEYS.geminiApiKey, patch.geminiApiKey)
  assignOrRemoveEnv(GEMINI_ENV_KEYS.googleApiKey, patch.googleApiKey)
  if (typeof patch.geminiApiKey === "string") {
    assignOrRemoveEnv(GEMINI_ENV_KEYS.legacyGeminiApiKey, "")
  }
  if (typeof patch.googleCloudProject === "string") {
    const project = patch.googleCloudProject.trim()
    if (!project) {
      delete env[GEMINI_ENV_KEYS.cloudProject]
      delete env[GEMINI_ENV_KEYS.cloudProjectLegacy]
    } else {
      env[GEMINI_ENV_KEYS.cloudProject] = project
      delete env[GEMINI_ENV_KEYS.cloudProjectLegacy]
    }
  }
  assignOrRemoveEnv(GEMINI_ENV_KEYS.cloudLocation, patch.googleCloudLocation)
  assignOrRemoveEnv(
    GEMINI_ENV_KEYS.applicationCredentials,
    patch.googleApplicationCredentials
  )

  if (Object.keys(env).length === 0) {
    delete config.env
  } else {
    config.env = env
  }

  return {
    configText:
      Object.keys(config).length === 0 ? "" : JSON.stringify(config, null, 2),
    recoveredFromInvalid: Boolean(parseResult.error),
  }
}

function patchGeminiEnvText(
  envText: string,
  patch: {
    apiBaseUrl?: string
    geminiApiKey?: string
    googleApiKey?: string
    googleCloudProject?: string
    googleCloudLocation?: string
    googleApplicationCredentials?: string
    model?: string
  }
): string {
  const envPatch: Record<string, string | undefined> = {}
  if (typeof patch.apiBaseUrl === "string") {
    envPatch[GEMINI_ENV_KEYS.baseUrl] = patch.apiBaseUrl
    envPatch[GEMINI_ENV_KEYS.legacyBaseUrl] = ""
  }
  if (typeof patch.geminiApiKey === "string") {
    envPatch[GEMINI_ENV_KEYS.geminiApiKey] = patch.geminiApiKey
    envPatch[GEMINI_ENV_KEYS.legacyGeminiApiKey] = ""
  }
  if (typeof patch.googleApiKey === "string") {
    envPatch[GEMINI_ENV_KEYS.googleApiKey] = patch.googleApiKey
  }
  if (typeof patch.googleCloudProject === "string") {
    envPatch[GEMINI_ENV_KEYS.cloudProject] = patch.googleCloudProject
    envPatch[GEMINI_ENV_KEYS.cloudProjectLegacy] = ""
  }
  if (typeof patch.googleCloudLocation === "string") {
    envPatch[GEMINI_ENV_KEYS.cloudLocation] = patch.googleCloudLocation
  }
  if (typeof patch.googleApplicationCredentials === "string") {
    envPatch[GEMINI_ENV_KEYS.applicationCredentials] =
      patch.googleApplicationCredentials
  }
  if (typeof patch.model === "string") {
    envPatch[GEMINI_ENV_KEYS.model] = patch.model
  }
  return patchEnvText(envText, envPatch)
}

function patchGeminiAuthMode(
  current: GeminiImportantValues,
  mode: GeminiAuthMode
) {
  const next = {
    ...current,
    authMode: mode,
  }
  if (mode === "login_google") {
    next.apiBaseUrl = ""
    next.geminiApiKey = ""
    next.googleApiKey = ""
    next.googleCloudProject = ""
    next.googleCloudLocation = ""
    next.googleApplicationCredentials = ""
    return next
  }
  if (mode === "custom") {
    next.googleApiKey = ""
    next.googleCloudProject = ""
    next.googleCloudLocation = ""
    next.googleApplicationCredentials = ""
    return next
  }
  if (mode === "gemini_api_key") {
    next.apiBaseUrl = ""
    next.googleApiKey = ""
    next.googleCloudProject = ""
    next.googleCloudLocation = ""
    next.googleApplicationCredentials = ""
    return next
  }
  if (mode === "vertex_api_key") {
    next.apiBaseUrl = ""
    next.geminiApiKey = ""
    next.googleApplicationCredentials = ""
    return next
  }
  if (mode === "vertex_service_account") {
    next.apiBaseUrl = ""
    next.geminiApiKey = ""
    next.googleApiKey = ""
    return next
  }
  if (mode === "model_provider") {
    next.googleCloudProject = ""
    next.googleCloudLocation = ""
    next.googleApplicationCredentials = ""
    return next
  }
  next.apiBaseUrl = ""
  next.geminiApiKey = ""
  next.googleApiKey = ""
  next.googleApplicationCredentials = ""
  return next
}

function geminiAuthModeLabel(mode: GeminiAuthMode): string {
  if (mode === "custom")
    return acpText("authModeCustomEndpoint", "Custom Endpoint")
  if (mode === "login_google")
    return acpText("gemini.mode.loginGoogle", "Google Login (OAuth)")
  if (mode === "gemini_api_key") return "Gemini API Key"
  if (mode === "vertex_adc") return "Vertex AI (ADC)"
  if (mode === "vertex_service_account")
    return acpText(
      "gemini.mode.vertexServiceAccount",
      "Vertex AI (Service Account)"
    )
  if (mode === "model_provider")
    return acpText("authModeModelProvider", "Model Provider")
  return "Vertex AI API Key"
}

function geminiAuthModeHint(mode: GeminiAuthMode): string {
  if (mode === "custom") {
    return acpText(
      "gemini.hint.custom",
      "Fill API URL, API Key and Model, mapped to GOOGLE_GEMINI_BASE_URL / GEMINI_API_KEY / GEMINI_MODEL."
    )
  }
  if (mode === "login_google") {
    return acpText(
      "gemini.hint.loginGoogle",
      "Run gemini in terminal and complete Google login first; API key is not required."
    )
  }
  if (mode === "gemini_api_key") {
    return acpText(
      "gemini.hint.geminiApiKey",
      "Fill GEMINI_API_KEY when using Gemini API."
    )
  }
  if (mode === "vertex_adc") {
    return acpText(
      "gemini.hint.vertexAdc",
      "Use gcloud ADC; GOOGLE_CLOUD_PROJECT and GOOGLE_CLOUD_LOCATION are recommended."
    )
  }
  if (mode === "vertex_service_account") {
    return acpText(
      "gemini.hint.vertexServiceAccount",
      "Set service account JSON path to GOOGLE_APPLICATION_CREDENTIALS."
    )
  }
  if (mode === "model_provider") {
    return acpText(
      "modelProviderHint",
      "Use API URL and API Key from a configured model provider."
    )
  }
  return acpText(
    "gemini.hint.vertexApiKey",
    "Fill GOOGLE_API_KEY when using Vertex AI API key."
  )
}

/**
 * Compare original and current config objects. For any key present in
 * original but missing in current, set it to `null` in the result so
 * the backend merge can delete it from the file on disk.
 */
function markRemovedKeysNull(
  original: Record<string, unknown>,
  current: Record<string, unknown>
): Record<string, unknown> {
  const result: Record<string, unknown> = { ...current }
  for (const key of Object.keys(original)) {
    if (!(key in result)) {
      result[key] = null
    } else if (
      original[key] &&
      typeof original[key] === "object" &&
      !Array.isArray(original[key]) &&
      result[key] &&
      typeof result[key] === "object" &&
      !Array.isArray(result[key])
    ) {
      result[key] = markRemovedKeysNull(
        original[key] as Record<string, unknown>,
        result[key] as Record<string, unknown>
      )
    }
  }
  return result
}

function normalizeConfigText(configText: string): string {
  const parseResult = parseConfigJsonText(configText)
  if (parseResult.error) return configText.trim()
  if (Object.keys(parseResult.config).length === 0) return ""
  return JSON.stringify(parseResult.config, null, 2)
}

interface OpenCodeProviderView {
  id: string
  name: string
  api: string
  npm: string
  baseUrl: string
  apiKey: string
  modelCount: number
  modelIds: string[]
  models: Record<string, OpenCodeModelView>
}

interface OpenCodeModelView {
  id: string
  name: string
  extraFieldCount: number
}

interface OpenCodeConfigView {
  model: string
  smallModel: string
  enabledProviders: string[]
  disabledProviders: string[]
  providerIds: string[]
  providers: Record<string, OpenCodeProviderView>
}

const OPENCODE_PROVIDER_NPM_OPTIONS = [
  {
    value: "@ai-sdk/openai-compatible",
    label: "@ai-sdk/openai-compatible",
  },
  {
    value: "@ai-sdk/cerebras",
    label: "@ai-sdk/cerebras",
  },
] as const

interface OpenCodeModelOptionGroup {
  providerId: string
  label: string
  models: { value: string; label: string }[]
}

function buildOpenCodeModelOptions(
  config: OpenCodeConfigView | null
): OpenCodeModelOptionGroup[] {
  if (!config) return []
  const groups: OpenCodeModelOptionGroup[] = []
  for (const providerId of config.providerIds) {
    const provider = config.providers[providerId]
    if (!provider || provider.modelIds.length === 0) continue
    groups.push({
      providerId,
      label: provider.name || providerId,
      models: provider.modelIds.map((modelId) => ({
        value: `${providerId}/${modelId}`,
        label: modelId,
      })),
    })
  }
  return groups
}

function OpenCodeModelCombobox({
  value,
  onValueChange,
  groups,
  placeholder,
}: {
  value: string
  onValueChange: (value: string) => void
  groups: OpenCodeModelOptionGroup[]
  placeholder: string
}) {
  const inputRef = useRef<HTMLInputElement>(null)

  const handleSelect = useCallback(
    (next: string | null) => {
      if (typeof next === "string" && next !== value) {
        onValueChange(next)
      }
    },
    [onValueChange, value]
  )

  const handleBlur = useCallback(() => {
    const trimmed = (inputRef.current?.value ?? "").trim()
    if (trimmed !== value) {
      onValueChange(trimmed)
    }
  }, [onValueChange, value])

  return (
    <Combobox key={value} value={value} onValueChange={handleSelect}>
      <ComboboxInput
        ref={inputRef}
        placeholder={placeholder}
        onBlur={handleBlur}
        showClear={false}
      />
      <ComboboxContent>
        <ComboboxList>
          {groups.map((group) => (
            <ComboboxGroup key={group.providerId}>
              <ComboboxLabel>{group.label}</ComboboxLabel>
              {group.models.map((model) => (
                <ComboboxItem key={model.value} value={model.value}>
                  {model.value}
                </ComboboxItem>
              ))}
            </ComboboxGroup>
          ))}
          <ComboboxEmpty>
            {acpText("openCode.noMatchingModels", "No matching models")}
          </ComboboxEmpty>
        </ComboboxList>
      </ComboboxContent>
    </Combobox>
  )
}

function buildOpenCodeNpmOptions(currentValue: string): string[] {
  const next = new Set<string>(
    OPENCODE_PROVIDER_NPM_OPTIONS.map((v) => v.value)
  )
  const current = currentValue.trim()
  if (current) next.add(current)
  return Array.from(next)
}

function extractOpenCodeConfigValues(
  configText: string,
  authJsonText: string
): OpenCodeConfigView {
  const parseResult = parseConfigJsonText(configText)
  const config = parseResult.error ? {} : parseResult.config
  const authParsed = parseOpenCodeAuthJsonText(authJsonText)
  const authObject = authParsed.authObject ?? {}
  const providerRoot = asObjectRecord(config.provider) ?? {}
  const providerIds = Object.keys(providerRoot)
  const providers: Record<string, OpenCodeProviderView> = {}
  const knownModelKeys = new Set(["id", "name"])

  for (const providerId of providerIds) {
    const rawProvider = asObjectRecord(providerRoot[providerId]) ?? {}
    const options = asObjectRecord(rawProvider.options) ?? {}
    const models = asObjectRecord(rawProvider.models) ?? {}
    const modelIds = Object.keys(models)
    const providerModels: Record<string, OpenCodeModelView> = {}
    for (const modelId of modelIds) {
      const rawModel = asObjectRecord(models[modelId]) ?? {}
      providerModels[modelId] = {
        // OpenCode uses `provider.models.<model_id>` as the true model id.
        id: modelId,
        name:
          pickFirstString(rawModel, ["name"]) ??
          pickFirstString(rawModel, ["id"]) ??
          "",
        extraFieldCount: Object.keys(rawModel).filter(
          (key) => !knownModelKeys.has(key)
        ).length,
      }
    }
    const authEntry = asObjectRecord(authObject[providerId]) ?? {}
    const authKey = pickFirstString(authEntry, ["key"]) ?? ""
    providers[providerId] = {
      id: providerId,
      name: pickFirstString(rawProvider, ["name"]) ?? "",
      api: pickFirstString(rawProvider, ["api"]) ?? "",
      npm: pickFirstString(rawProvider, ["npm"]) ?? "",
      baseUrl: pickFirstString(options, ["baseURL", "baseUrl"]) ?? "",
      apiKey: pickFirstString(options, ["apiKey", "api_key"]) ?? authKey,
      modelCount: modelIds.length,
      modelIds,
      models: providerModels,
    }
  }

  return {
    model: pickFirstString(config, ["model"]) ?? "",
    smallModel:
      pickFirstString(config, ["small_model", "smallModel", "small-model"]) ??
      "",
    enabledProviders: Array.isArray(config.enabled_providers)
      ? config.enabled_providers
          .filter((item): item is string => typeof item === "string")
          .map((item) => item.trim())
          .filter(Boolean)
      : [],
    disabledProviders: Array.isArray(config.disabled_providers)
      ? config.disabled_providers
          .filter((item): item is string => typeof item === "string")
          .map((item) => item.trim())
          .filter(Boolean)
      : [],
    providerIds,
    providers,
  }
}

function patchOpenCodeConfigText(
  configText: string,
  mutator: (config: Record<string, unknown>) => void
): {
  configText: string
  recoveredFromInvalid: boolean
} {
  const parseResult = parseConfigJsonText(configText)
  const config = parseResult.error
    ? {}
    : (JSON.parse(JSON.stringify(parseResult.config)) as Record<
        string,
        unknown
      >)
  mutator(config)
  return {
    configText:
      Object.keys(config).length === 0 ? "" : JSON.stringify(config, null, 2),
    recoveredFromInvalid: Boolean(parseResult.error),
  }
}

interface CodexTomlImportantValues {
  model: string
  modelProvider: string
  modelReasoningEffort: CodexReasoningEffort
  providerNames: string[]
  providerBaseUrls: Record<string, string>
  providerSupportsWebsockets: Record<string, boolean>
  featureResponsesWebsocketsV2: boolean
}

interface CodexImportantValues {
  apiBaseUrl: string
  apiKey: string | null
  model: string
  modelProvider: string
  reasoningEffort: CodexReasoningEffort
  providerOptions: string[]
  supportsWebsockets: boolean
}

const CODEX_DEFAULT_MODEL_PROVIDER = "codeg"

const CODEX_AUTH_MODES = [
  "api_key",
  "chatgpt_subscription",
  "model_provider",
] as const
type CodexAuthMode = (typeof CODEX_AUTH_MODES)[number]

type CodexReasoningEffort = "low" | "medium" | "high" | "xhigh"

const CODEX_REASONING_EFFORT_OPTIONS: ReadonlyArray<{
  value: CodexReasoningEffort
  label: string
  description: string
}> = [
  {
    value: "low",
    label: "Low",
    description: "Fast responses with lighter reasoning",
  },
  {
    value: "medium",
    label: "Medium",
    description: "Balances speed and reasoning depth for everyday tasks",
  },
  {
    value: "high",
    label: "High",
    description: "Greater reasoning depth for complex problems",
  },
  {
    value: "xhigh",
    label: "Extra High",
    description: "Extra high reasoning depth for complex problems",
  },
]

const CODEX_DEFAULT_REASONING_EFFORT: CodexReasoningEffort = "high"

function normalizeCodexReasoningEffort(
  value: string
): CodexReasoningEffort | null {
  const normalized = value.trim().toLowerCase()
  if (
    normalized === "low" ||
    normalized === "medium" ||
    normalized === "high" ||
    normalized === "xhigh"
  ) {
    return normalized
  }
  return null
}

function buildCodexProviderOptions(
  activeProvider: string,
  providerNames: string[]
): string[] {
  const result: string[] = []
  const seen = new Set<string>()
  for (const raw of [
    activeProvider,
    ...providerNames,
    CODEX_DEFAULT_MODEL_PROVIDER,
  ]) {
    const provider = raw.trim()
    if (!provider || seen.has(provider)) continue
    seen.add(provider)
    result.push(provider)
  }
  return result
}

function parseTomlStringLiteral(raw: string): string | null {
  const text = raw.trim()
  if (!text) return null

  if (text.startsWith('"')) {
    let escaped = false
    for (let i = 1; i < text.length; i += 1) {
      const ch = text[i]
      if (escaped) {
        escaped = false
        continue
      }
      if (ch === "\\") {
        escaped = true
        continue
      }
      if (ch === '"') {
        const literal = text.slice(0, i + 1)
        try {
          return JSON.parse(literal) as string
        } catch {
          return literal.slice(1, -1)
        }
      }
    }
    return null
  }

  if (text.startsWith("'")) {
    const end = text.indexOf("'", 1)
    if (end <= 0) return null
    return text.slice(1, end)
  }

  return null
}

function parseTomlStringAssignment(
  rawLine: string
): { key: string; value: string } | null {
  const key = parseTomlAssignmentKey(rawLine)
  if (!key) return null
  const line = rawLine.trim()
  const equalsIndex = line.indexOf("=")
  const valueText = line.slice(equalsIndex + 1)
  const value = parseTomlStringLiteral(valueText)
  if (value === null) return null
  return { key, value: value.trim() }
}

function parseTomlAssignmentKey(rawLine: string): string | null {
  const line = rawLine.trim()
  if (!line || line.startsWith("#")) return null
  const equalsIndex = line.indexOf("=")
  if (equalsIndex <= 0) return null
  const key = line.slice(0, equalsIndex).trim()
  if (!/^[A-Za-z0-9_.-]+$/.test(key)) return null
  return key
}

function parseTomlBooleanAssignment(
  rawLine: string
): { key: string; value: boolean } | null {
  const key = parseTomlAssignmentKey(rawLine)
  if (!key) return null
  const line = rawLine.trim()
  const equalsIndex = line.indexOf("=")
  const valueText = line.slice(equalsIndex + 1).trim()
  const boolMatch = valueText.match(/^(true|false)(?:\s+#.*)?$/)
  if (!boolMatch) return null
  return { key, value: boolMatch[1] === "true" }
}

function extractCodexTomlImportantValues(
  configTomlText: string
): CodexTomlImportantValues {
  const providerBaseUrls: Record<string, string> = {}
  const providerSupportsWebsockets: Record<string, boolean> = {}
  const providerNames = new Set<string>()
  let model = ""
  let modelProvider = ""
  let modelReasoningEffort: CodexReasoningEffort =
    CODEX_DEFAULT_REASONING_EFFORT
  let featureResponsesWebsocketsV2 = false
  let currentProviderSection: string | null = null
  let inFeaturesSection = false

  for (const rawLine of configTomlText.split(/\r?\n/)) {
    const line = rawLine.trim()
    if (!line || line.startsWith("#")) continue

    const sectionMatch = line.match(
      /^\[\s*model_providers\.([A-Za-z0-9_-]+)\s*\]$/
    )
    if (sectionMatch) {
      currentProviderSection = sectionMatch[1]
      inFeaturesSection = false
      if (currentProviderSection.trim()) {
        providerNames.add(currentProviderSection.trim())
      }
      continue
    }
    if (line.match(/^\[\s*features\s*\]$/)) {
      inFeaturesSection = true
      currentProviderSection = null
      continue
    }
    if (line.startsWith("[") && line.endsWith("]")) {
      currentProviderSection = null
      inFeaturesSection = false
      continue
    }

    const assignment = parseTomlStringAssignment(rawLine)
    if (assignment) {
      if (assignment.key === "model") {
        model = assignment.value
        continue
      }
      if (assignment.key === "model_provider") {
        modelProvider = assignment.value
        continue
      }
      if (assignment.key === "model_reasoning_effort") {
        modelReasoningEffort =
          normalizeCodexReasoningEffort(assignment.value) ??
          CODEX_DEFAULT_REASONING_EFFORT
        continue
      }
    }

    const boolAssignment = parseTomlBooleanAssignment(rawLine)
    if (boolAssignment) {
      if (
        currentProviderSection &&
        boolAssignment.key === "supports_websockets"
      ) {
        providerSupportsWebsockets[currentProviderSection] =
          boolAssignment.value
        providerNames.add(currentProviderSection.trim())
        continue
      }
      if (
        inFeaturesSection &&
        boolAssignment.key === "responses_websockets_v2"
      ) {
        featureResponsesWebsocketsV2 = boolAssignment.value
        continue
      }
      const dottedProviderWebsocketMatch = boolAssignment.key.match(
        /^model_providers\.([A-Za-z0-9_-]+)\.supports_websockets$/
      )
      if (dottedProviderWebsocketMatch && dottedProviderWebsocketMatch[1]) {
        const providerName = dottedProviderWebsocketMatch[1].trim()
        providerNames.add(providerName)
        providerSupportsWebsockets[providerName] = boolAssignment.value
        continue
      }
      if (boolAssignment.key === "features.responses_websockets_v2") {
        featureResponsesWebsocketsV2 = boolAssignment.value
        continue
      }
    }

    if (!assignment) continue

    const rawAssignmentKey = parseTomlAssignmentKey(rawLine)
    const dottedProviderMatch = rawAssignmentKey?.match(
      /^model_providers\.([A-Za-z0-9_-]+)\./
    )
    if (dottedProviderMatch && dottedProviderMatch[1]) {
      providerNames.add(dottedProviderMatch[1].trim())
    }
    if (
      currentProviderSection &&
      assignment.key === "base_url" &&
      assignment.value
    ) {
      providerBaseUrls[currentProviderSection] = assignment.value
      providerNames.add(currentProviderSection.trim())
      continue
    }
    const dottedMatch = assignment.key.match(
      /^model_providers\.([A-Za-z0-9_-]+)\.base_url$/
    )
    if (dottedMatch && assignment.value) {
      providerBaseUrls[dottedMatch[1]] = assignment.value
      providerNames.add(dottedMatch[1].trim())
    }
  }
  if (modelProvider.trim()) {
    providerNames.add(modelProvider.trim())
  }
  providerNames.add(CODEX_DEFAULT_MODEL_PROVIDER)
  for (const providerName of Object.keys(providerBaseUrls)) {
    if (providerName.trim()) {
      providerNames.add(providerName.trim())
    }
  }

  return {
    model,
    modelProvider,
    modelReasoningEffort,
    providerNames: Array.from(providerNames),
    providerBaseUrls,
    providerSupportsWebsockets,
    featureResponsesWebsocketsV2,
  }
}

function parseCodexAuthJsonObject(authJsonText: string): {
  authObject: Record<string, unknown> | null
  error: string | null
} {
  const trimmed = authJsonText.trim()
  if (!trimmed) return { authObject: {}, error: null }
  try {
    const parsed = JSON.parse(trimmed) as unknown
    if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
      return {
        authObject: null,
        error: acpText(
          "errors.authMustBeObject",
          "auth.json must be a JSON object"
        ),
      }
    }
    return { authObject: parsed as Record<string, unknown>, error: null }
  } catch (err) {
    const message = err instanceof Error ? err.message : String(err)
    return {
      authObject: null,
      error: acpText(
        "errors.authInvalid",
        "auth.json format error: {message}",
        {
          message,
        }
      ),
    }
  }
}

function parseCodexAuthJsonText(authJsonText: string): string | null {
  return parseCodexAuthJsonObject(authJsonText).error
}

function inferCodexAuthMode(authJsonText: string): CodexAuthMode {
  const { authObject } = parseCodexAuthJsonObject(authJsonText)
  if (authObject) {
    // 官网订阅：auth_mode 为 chatgpt，或没有 OPENAI_API_KEY，或值为 null
    if (
      authObject.auth_mode === "chatgpt" ||
      !("OPENAI_API_KEY" in authObject) ||
      authObject.OPENAI_API_KEY === null
    ) {
      return "chatgpt_subscription"
    }
  }
  return "api_key"
}

function extractCodexImportantValues(
  authJsonText: string,
  configTomlText: string
): CodexImportantValues {
  const parsedAuth = parseCodexAuthJsonObject(authJsonText)
  const authObject = parsedAuth.authObject ?? {}
  const toml = extractCodexTomlImportantValues(configTomlText)
  const hasExplicitProvider = Boolean(toml.modelProvider.trim())
  const activeProvider = hasExplicitProvider
    ? toml.modelProvider.trim()
    : CODEX_DEFAULT_MODEL_PROVIDER
  const providerBaseUrl = hasExplicitProvider
    ? (toml.providerBaseUrls[activeProvider] ?? "")
    : (toml.providerBaseUrls[CODEX_DEFAULT_MODEL_PROVIDER] ??
      toml.providerBaseUrls.openai ??
      "")
  const providerSupportsWebsockets =
    toml.providerSupportsWebsockets[activeProvider] ??
    (activeProvider === CODEX_DEFAULT_MODEL_PROVIDER
      ? toml.featureResponsesWebsocketsV2
      : false)
  return {
    apiBaseUrl: providerBaseUrl,
    apiKey:
      parsedAuth.error === null
        ? (pickFirstString(authObject, [
            "OPENAI_API_KEY",
            "OPENAI_API_TOKEN",
            "API_KEY",
          ]) ?? "")
        : null,
    model: toml.model,
    modelProvider: activeProvider,
    reasoningEffort: toml.modelReasoningEffort,
    providerOptions: buildCodexProviderOptions(
      activeProvider,
      toml.providerNames
    ),
    supportsWebsockets: providerSupportsWebsockets,
  }
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")
}

function findTomlRootEndIndex(lines: string[]): number {
  for (let i = 0; i < lines.length; i += 1) {
    if (/^\[.*\]$/.test(lines[i].trim())) return i
  }
  return lines.length
}

function findTomlRootAssignmentIndex(lines: string[], key: string): number {
  const rootEnd = findTomlRootEndIndex(lines)
  for (let i = 0; i < rootEnd; i += 1) {
    const assignmentKey = parseTomlAssignmentKey(lines[i])
    if (assignmentKey === key) return i
  }
  return -1
}

function preferredTomlRootInsertionIndex(lines: string[], key: string): number {
  if (key === "model") {
    const providerIndex = findTomlRootAssignmentIndex(lines, "model_provider")
    return providerIndex >= 0 ? providerIndex : 0
  }
  if (key === "model_reasoning_effort") {
    const modelIndex = findTomlRootAssignmentIndex(lines, "model")
    return modelIndex >= 0 ? modelIndex + 1 : 0
  }
  return findTomlRootEndIndex(lines)
}

function updateTomlRootStringKey(
  configTomlText: string,
  key: string,
  value: string
): string {
  const lineText = `${key} = ${JSON.stringify(value)}`
  const lines = configTomlText.split(/\r?\n/)
  const assignmentIndex = findTomlRootAssignmentIndex(lines, key)

  const nextValue = value.trim()
  if (!nextValue) {
    if (assignmentIndex >= 0) {
      lines.splice(assignmentIndex, 1)
    }
    return lines.join("\n").trim()
  }

  const insertAt = preferredTomlRootInsertionIndex(lines, key)
  if (assignmentIndex >= 0) {
    lines[assignmentIndex] = lineText
  } else {
    lines.splice(Math.max(0, insertAt), 0, lineText)
  }
  return lines.join("\n").trim()
}

function updateTomlRootBooleanKey(
  configTomlText: string,
  key: string,
  value: boolean
): string {
  const lineText = `${key} = ${value ? "true" : "false"}`
  const lines = configTomlText.split(/\r?\n/)
  const assignmentIndex = findTomlRootAssignmentIndex(lines, key)
  if (assignmentIndex >= 0) {
    lines[assignmentIndex] = lineText
  } else {
    lines.splice(0, 0, lineText)
  }
  return lines.join("\n").trim()
}

function findTomlSectionRange(
  lines: string[],
  sectionName: string
): { start: number; end: number } | null {
  const headerText = `[${sectionName}]`
  let sectionStart = -1
  let sectionEnd = lines.length
  for (let i = 0; i < lines.length; i += 1) {
    const trimmed = lines[i].trim()
    if (sectionStart < 0) {
      if (trimmed === headerText) {
        sectionStart = i
      }
      continue
    }
    if (/^\[.*\]$/.test(trimmed)) {
      sectionEnd = i
      break
    }
  }
  if (sectionStart < 0) return null
  return { start: sectionStart, end: sectionEnd }
}

function removeTomlSection(
  configTomlText: string,
  sectionName: string
): string {
  const lines = configTomlText.split(/\r?\n/)
  const range = findTomlSectionRange(lines, sectionName)
  if (!range) return configTomlText
  // Remove blank line before section header if present
  const removeStart =
    range.start > 0 && lines[range.start - 1].trim() === ""
      ? range.start - 1
      : range.start
  lines.splice(removeStart, range.end - removeStart)
  return lines.join("\n").trim()
}

function upsertTomlSectionBooleanKey(
  configTomlText: string,
  sectionName: string,
  key: string,
  value: boolean | null
): string {
  const lines = configTomlText.split(/\r?\n/)
  const section = findTomlSectionRange(lines, sectionName)

  if (section) {
    let assignmentIndex = -1
    for (let i = section.start + 1; i < section.end; i += 1) {
      const assignmentKey = parseTomlAssignmentKey(lines[i])
      if (assignmentKey === key) {
        assignmentIndex = i
        break
      }
    }

    if (value === null) {
      if (assignmentIndex >= 0) {
        lines.splice(assignmentIndex, 1)
      }
      const refreshedSection = findTomlSectionRange(lines, sectionName)
      if (refreshedSection) {
        const hasEntries = lines
          .slice(refreshedSection.start + 1, refreshedSection.end)
          .some((rawLine) => {
            const line = rawLine.trim()
            return line !== "" && !line.startsWith("#")
          })
        if (!hasEntries) {
          const before = lines.slice(0, refreshedSection.start)
          const after = lines.slice(refreshedSection.end)
          while (before.length > 0 && before[before.length - 1].trim() === "") {
            before.pop()
          }
          while (after.length > 0 && after[0].trim() === "") {
            after.shift()
          }
          const merged =
            before.length > 0 && after.length > 0
              ? [...before, "", ...after]
              : [...before, ...after]
          return merged.join("\n").trim()
        }
      }
      return lines.join("\n").trim()
    }

    const lineText = `${key} = ${value ? "true" : "false"}`
    if (assignmentIndex >= 0) {
      lines[assignmentIndex] = lineText
    } else {
      lines.splice(section.end, 0, lineText)
    }
    return lines.join("\n").trim()
  }

  if (value === null) {
    return configTomlText.trim()
  }

  const lineText = `${key} = ${value ? "true" : "false"}`
  const insertAt = findTomlRootEndIndex(lines)
  const prefixBlank =
    insertAt > 0 && lines[insertAt - 1].trim() !== "" ? [""] : []
  const suffixBlank =
    insertAt < lines.length && lines[insertAt].trim() !== "" ? [""] : []
  lines.splice(
    insertAt,
    0,
    ...prefixBlank,
    `[${sectionName}]`,
    lineText,
    ...suffixBlank
  )
  return lines.join("\n").trim()
}

function patchCodexProviderBaseUrl(
  configTomlText: string,
  provider: string,
  apiBaseUrl: string
): string {
  const trimmedProvider = provider.trim()
  if (!trimmedProvider) return configTomlText.trim()

  const nextApiBaseUrl = apiBaseUrl.trim()
  const lines = configTomlText.split(/\r?\n/)
  const sectionPattern = new RegExp(
    `^\\[\\s*model_providers\\.${escapeRegExp(trimmedProvider)}\\s*\\]$`
  )
  let sectionStart = -1
  let sectionEnd = lines.length
  for (let i = 0; i < lines.length; i += 1) {
    const trimmed = lines[i].trim()
    if (sectionStart < 0) {
      if (sectionPattern.test(trimmed)) {
        sectionStart = i
      }
      continue
    }
    if (/^\[.*\]$/.test(trimmed)) {
      sectionEnd = i
      break
    }
  }

  if (sectionStart >= 0) {
    let baseUrlIndex = -1
    for (let i = sectionStart + 1; i < sectionEnd; i += 1) {
      const assignment = parseTomlStringAssignment(lines[i])
      if (!assignment || assignment.key !== "base_url") continue
      baseUrlIndex = i
      break
    }
    if (!nextApiBaseUrl) {
      if (baseUrlIndex >= 0) {
        lines.splice(baseUrlIndex, 1)
      }
      return lines.join("\n").trim()
    }

    const lineText = `base_url = ${JSON.stringify(nextApiBaseUrl)}`
    if (baseUrlIndex >= 0) {
      lines[baseUrlIndex] = lineText
    } else {
      lines.splice(sectionEnd, 0, lineText)
    }
    return lines.join("\n").trim()
  }

  if (!nextApiBaseUrl) return configTomlText.trim()

  const appended = configTomlText.trimEnd()
  const sectionText = `[model_providers.${trimmedProvider}]\nbase_url = ${JSON.stringify(nextApiBaseUrl)}`
  if (!appended) return sectionText
  return `${appended}\n\n${sectionText}`.trim()
}

function patchCodexProviderField(
  configTomlText: string,
  provider: string,
  key: string,
  lineText: string
): string {
  const trimmedProvider = provider.trim()
  if (!trimmedProvider) return configTomlText.trim()

  const lines = configTomlText.split(/\r?\n/)
  const sectionPattern = new RegExp(
    `^\\[\\s*model_providers\\.${escapeRegExp(trimmedProvider)}\\s*\\]$`
  )
  let sectionStart = -1
  let sectionEnd = lines.length
  for (let i = 0; i < lines.length; i += 1) {
    const trimmed = lines[i].trim()
    if (sectionStart < 0) {
      if (sectionPattern.test(trimmed)) {
        sectionStart = i
      }
      continue
    }
    if (/^\[.*\]$/.test(trimmed)) {
      sectionEnd = i
      break
    }
  }

  if (sectionStart >= 0) {
    let fieldIndex = -1
    for (let i = sectionStart + 1; i < sectionEnd; i += 1) {
      const assignmentKey = parseTomlAssignmentKey(lines[i])
      if (assignmentKey !== key) continue
      fieldIndex = i
      break
    }
    if (fieldIndex >= 0) {
      lines[fieldIndex] = lineText
    } else {
      let insertAt = sectionEnd
      while (insertAt > sectionStart + 1 && lines[insertAt - 1].trim() === "") {
        insertAt -= 1
      }
      lines.splice(insertAt, 0, lineText)
    }
    return lines.join("\n").trim()
  }

  const appended = configTomlText.trimEnd()
  const sectionText = `[model_providers.${trimmedProvider}]\n${lineText}`
  if (!appended) return sectionText
  return `${appended}\n\n${sectionText}`.trim()
}

function ensureCodexProviderDefaults(
  configTomlText: string,
  provider: string
): string {
  if (provider.trim() !== CODEX_DEFAULT_MODEL_PROVIDER) {
    return configTomlText
  }
  let next = configTomlText
  const current = extractCodexTomlImportantValues(next)
  const codegBaseUrl =
    current.providerBaseUrls[CODEX_DEFAULT_MODEL_PROVIDER] ?? ""
  next = patchCodexProviderField(
    next,
    CODEX_DEFAULT_MODEL_PROVIDER,
    "base_url",
    `base_url = ${JSON.stringify(codegBaseUrl)}`
  )
  next = patchCodexProviderField(
    next,
    CODEX_DEFAULT_MODEL_PROVIDER,
    "name",
    'name = "codeg"'
  )
  next = patchCodexProviderField(
    next,
    CODEX_DEFAULT_MODEL_PROVIDER,
    "wire_api",
    'wire_api = "responses"'
  )
  next = patchCodexProviderField(
    next,
    CODEX_DEFAULT_MODEL_PROVIDER,
    "requires_openai_auth",
    "requires_openai_auth = true"
  )
  return next
}

function patchCodexAuthJsonText(
  authJsonText: string,
  patch: { apiKey?: string; authMode?: "chatgpt" | null }
): {
  authJsonText: string
  recoveredFromInvalid: boolean
} {
  const parsed = parseCodexAuthJsonObject(authJsonText)
  const authObject =
    parsed.error === null && parsed.authObject ? { ...parsed.authObject } : {}
  if (typeof patch.apiKey === "string") {
    const apiKey = patch.apiKey.trim()
    if (apiKey) {
      authObject.OPENAI_API_KEY = apiKey
      delete authObject.API_KEY
    } else {
      delete authObject.OPENAI_API_KEY
      delete authObject.OPENAI_API_TOKEN
      delete authObject.API_KEY
    }
  }
  if ("authMode" in patch) {
    if (patch.authMode === "chatgpt") {
      authObject.auth_mode = "chatgpt"
      authObject.OPENAI_API_KEY = null
    } else {
      delete authObject.auth_mode
    }
  }
  return {
    authJsonText:
      Object.keys(authObject).length === 0
        ? ""
        : JSON.stringify(authObject, null, 2),
    recoveredFromInvalid: Boolean(parsed.error),
  }
}

function patchCodexConfigTomlText(
  configTomlText: string,
  patch: {
    apiBaseUrl?: string
    model?: string
    modelProvider?: string
    modelReasoningEffort?: string
    supportsWebsockets?: boolean
  }
): string {
  let nextTomlText = configTomlText
  if (typeof patch.modelProvider === "string") {
    const modelProvider = patch.modelProvider.trim()
    if (modelProvider) {
      nextTomlText = updateTomlRootStringKey(
        nextTomlText,
        "model_provider",
        modelProvider
      )
      nextTomlText = ensureCodexProviderDefaults(nextTomlText, modelProvider)
    }
  }
  if (typeof patch.model === "string") {
    nextTomlText = updateTomlRootStringKey(nextTomlText, "model", patch.model)
  }
  if (typeof patch.modelReasoningEffort === "string") {
    const reasoningEffort =
      normalizeCodexReasoningEffort(patch.modelReasoningEffort) ??
      CODEX_DEFAULT_REASONING_EFFORT
    nextTomlText = updateTomlRootStringKey(
      nextTomlText,
      "model_reasoning_effort",
      reasoningEffort
    )
  }
  if (typeof patch.apiBaseUrl === "string") {
    const tomlValues = extractCodexTomlImportantValues(nextTomlText)
    const modelProvider =
      patch.modelProvider?.trim() ||
      tomlValues.modelProvider.trim() ||
      CODEX_DEFAULT_MODEL_PROVIDER
    if (!tomlValues.modelProvider.trim() && patch.apiBaseUrl.trim()) {
      nextTomlText = updateTomlRootStringKey(
        nextTomlText,
        "model_provider",
        modelProvider
      )
    }
    nextTomlText = patchCodexProviderBaseUrl(
      nextTomlText,
      modelProvider,
      patch.apiBaseUrl
    )
    nextTomlText = ensureCodexProviderDefaults(nextTomlText, modelProvider)
  }
  if (typeof patch.supportsWebsockets === "boolean") {
    const tomlValues = extractCodexTomlImportantValues(nextTomlText)
    const modelProvider =
      patch.modelProvider?.trim() ||
      tomlValues.modelProvider.trim() ||
      CODEX_DEFAULT_MODEL_PROVIDER
    if (!tomlValues.modelProvider.trim()) {
      nextTomlText = updateTomlRootStringKey(
        nextTomlText,
        "model_provider",
        modelProvider
      )
    }
    nextTomlText = patchCodexProviderField(
      nextTomlText,
      modelProvider,
      "supports_websockets",
      `supports_websockets = ${patch.supportsWebsockets ? "true" : "false"}`
    )
    nextTomlText = ensureCodexProviderDefaults(nextTomlText, modelProvider)
  }
  const normalizedTomlValues = extractCodexTomlImportantValues(nextTomlText)
  if (normalizedTomlValues.model.trim()) {
    nextTomlText = updateTomlRootStringKey(
      nextTomlText,
      "model",
      normalizedTomlValues.model
    )
  }
  nextTomlText = updateTomlRootStringKey(
    nextTomlText,
    "model_reasoning_effort",
    normalizedTomlValues.modelReasoningEffort
  )
  const activeProvider =
    normalizedTomlValues.modelProvider.trim() || CODEX_DEFAULT_MODEL_PROVIDER
  const shouldEnableFeature = Boolean(
    normalizedTomlValues.providerSupportsWebsockets[activeProvider]
  )
  nextTomlText = upsertTomlSectionBooleanKey(
    nextTomlText,
    "features",
    "responses_websockets_v2",
    shouldEnableFeature ? true : null
  )
  nextTomlText = updateTomlRootBooleanKey(
    nextTomlText,
    "disable_response_storage",
    true
  )
  const trimmed = nextTomlText.trim()
  return trimmed ? `${trimmed}\n` : ""
}

function patchImportantConfigText(
  agentType: AgentType,
  configText: string,
  patch: ImportantDraftPatch
): {
  configText: string
  recoveredFromInvalid: boolean
} {
  const parseResult = parseConfigJsonText(configText)
  const config = parseResult.error ? {} : { ...parseResult.config }

  const assignOrRemove = (key: string, value: string | undefined) => {
    const trimmed = value?.trim() ?? ""
    if (!trimmed) {
      delete config[key]
      return
    }
    config[key] = trimmed
  }

  if (agentType === "claude_code") {
    // Claude Code: write apiBaseUrl/apiKey into config.env, not root
    const env =
      typeof config.env === "object" && config.env && !Array.isArray(config.env)
        ? { ...(config.env as Record<string, unknown>) }
        : {}
    const assignEnv = (key: string, value: string | undefined) => {
      const trimmed = value?.trim() ?? ""
      if (!trimmed) {
        delete env[key]
        return
      }
      env[key] = trimmed
    }
    // Remove root-level apiBaseUrl/apiKey if present (legacy cleanup)
    delete config.apiBaseUrl
    delete config.apiKey
    assignEnv("ANTHROPIC_BASE_URL", patch.apiBaseUrl)
    assignEnv("ANTHROPIC_AUTH_TOKEN", patch.apiKey)

    assignEnv(CLAUDE_MODEL_ENV_KEYS.claudeMainModel, patch.claudeMainModel)
    assignEnv(
      CLAUDE_MODEL_ENV_KEYS.claudeReasoningModel,
      patch.claudeReasoningModel
    )
    assignEnv(
      CLAUDE_MODEL_ENV_KEYS.claudeDefaultHaikuModel,
      patch.claudeDefaultHaikuModel
    )
    assignEnv(
      CLAUDE_MODEL_ENV_KEYS.claudeDefaultSonnetModel,
      patch.claudeDefaultSonnetModel
    )
    assignEnv(
      CLAUDE_MODEL_ENV_KEYS.claudeDefaultOpusModel,
      patch.claudeDefaultOpusModel
    )

    if (Object.keys(env).length === 0) {
      delete config.env
    } else {
      config.env = env
    }
  } else {
    assignOrRemove("apiBaseUrl", patch.apiBaseUrl)
    assignOrRemove("apiKey", patch.apiKey)
    assignOrRemove("model", patch.model)
  }

  return {
    configText:
      Object.keys(config).length === 0 ? "" : JSON.stringify(config, null, 2),
    recoveredFromInvalid: Boolean(parseResult.error),
  }
}

function patchEnvByImportantKey(
  agentType: AgentType,
  envText: string,
  key: ImportantConfigKey,
  value: string
): string {
  const keys = importantEnvKeysByAgent(agentType)
  if (key === "apiBaseUrl") {
    return patchEnvText(envText, { [keys.apiBaseUrl[0]]: value })
  }
  if (key === "apiKey") {
    return patchEnvText(envText, { [keys.apiKey[0]]: value })
  }
  if (key === "model") {
    return patchEnvText(envText, { [keys.model[0]]: value })
  }
  return patchEnvText(envText, { [CLAUDE_MODEL_ENV_KEYS[key]]: value })
}

function applyImportantFieldToDraft(
  draft: AgentDraft,
  key: ImportantConfigKey,
  value: string
): AgentDraft {
  if (key === "apiBaseUrl") return { ...draft, apiBaseUrl: value }
  if (key === "apiKey") return { ...draft, apiKey: value }
  if (key === "model") return { ...draft, model: value }
  if (key === "claudeMainModel") return { ...draft, claudeMainModel: value }
  if (key === "claudeReasoningModel") {
    return { ...draft, claudeReasoningModel: value }
  }
  if (key === "claudeDefaultHaikuModel") {
    return { ...draft, claudeDefaultHaikuModel: value }
  }
  if (key === "claudeDefaultSonnetModel") {
    return { ...draft, claudeDefaultSonnetModel: value }
  }
  return { ...draft, claudeDefaultOpusModel: value }
}

function buildImportantPatchFromDraft(draft: AgentDraft): ImportantDraftPatch {
  return {
    apiBaseUrl: draft.apiBaseUrl,
    apiKey: draft.apiKey,
    model: draft.model,
    claudeMainModel: draft.claudeMainModel,
    claudeReasoningModel: draft.claudeReasoningModel,
    claudeDefaultHaikuModel: draft.claudeDefaultHaikuModel,
    claudeDefaultSonnetModel: draft.claudeDefaultSonnetModel,
    claudeDefaultOpusModel: draft.claudeDefaultOpusModel,
  }
}

function buildAgentDraft(agent: AcpAgentInfo): AgentDraft {
  const configText =
    typeof agent.config_json === "string" && agent.config_json.trim()
      ? agent.config_json
      : ""
  const openCodeAuthJsonText = agent.opencode_auth_json ?? ""
  const codexAuthJsonText = agent.codex_auth_json ?? ""
  const codexConfigTomlText =
    agent.agent_type === "codex"
      ? updateTomlRootBooleanKey(
          agent.codex_config_toml ?? "",
          "disable_response_storage",
          true
        )
      : (agent.codex_config_toml ?? "")
  const important = extractImportantConfigValues(
    agent.agent_type,
    agent.env,
    configText
  )
  const geminiImportant = extractGeminiImportantValues(agent.env, configText)
  const openClawImportant = extractOpenClawImportantValues(
    agent.env,
    configText
  )
  const codexImportant = extractCodexImportantValues(
    codexAuthJsonText,
    codexConfigTomlText
  )
  const openCodeImportant = extractOpenCodeConfigValues(
    configText,
    openCodeAuthJsonText
  )
  const clineImportant = extractClineImportantValues(configText)
  const codexAuthMode: CodexAuthMode =
    agent.agent_type === "codex" && agent.model_provider_id != null
      ? "model_provider"
      : agent.agent_type === "codex"
        ? inferCodexAuthMode(codexAuthJsonText)
        : "api_key"
  const rawEnvText = envMapToText(agent.env)
  // When codex is in official subscription mode, clean up API keys/URLs from env
  const envText =
    agent.agent_type === "codex" && codexAuthMode === "chatgpt_subscription"
      ? patchEnvText(rawEnvText, {
          OPENAI_API_KEY: "",
          OPENAI_BASE_URL: "",
        })
      : rawEnvText
  return {
    enabled: agent.enabled,
    envText,
    configText,
    apiBaseUrl:
      agent.agent_type === "codex"
        ? codexImportant.apiBaseUrl
        : agent.agent_type === "gemini"
          ? geminiImportant.apiBaseUrl
          : important.apiBaseUrl,
    apiKey:
      agent.agent_type === "codex"
        ? (codexImportant.apiKey ?? "")
        : agent.agent_type === "gemini"
          ? geminiImportant.geminiApiKey || geminiImportant.googleApiKey
          : important.apiKey,
    model:
      agent.agent_type === "codex"
        ? codexImportant.model
        : agent.agent_type === "gemini"
          ? geminiImportant.model
          : agent.agent_type === "open_code"
            ? openCodeImportant.model
            : important.model,
    claudeAuthMode:
      agent.agent_type === "claude_code" && agent.model_provider_id != null
        ? "model_provider"
        : agent.agent_type === "claude_code" &&
            (important.apiBaseUrl || important.apiKey)
          ? "custom"
          : "official_subscription",
    modelProviderId: agent.model_provider_id ?? null,
    geminiAuthMode:
      agent.agent_type === "gemini" && agent.model_provider_id != null
        ? "model_provider"
        : geminiImportant.authMode,
    geminiApiKey: geminiImportant.geminiApiKey,
    googleApiKey: geminiImportant.googleApiKey,
    googleCloudProject: geminiImportant.googleCloudProject,
    googleCloudLocation: geminiImportant.googleCloudLocation,
    googleApplicationCredentials: geminiImportant.googleApplicationCredentials,
    codexAuthMode,
    codexModelProvider: codexImportant.modelProvider,
    codexProviderOptions: codexImportant.providerOptions,
    codexReasoningEffort: codexImportant.reasoningEffort,
    codexSupportsWebsockets: codexImportant.supportsWebsockets,
    claudeMainModel: important.claudeMainModel,
    claudeReasoningModel: important.claudeReasoningModel,
    claudeDefaultHaikuModel: important.claudeDefaultHaikuModel,
    claudeDefaultSonnetModel: important.claudeDefaultSonnetModel,
    claudeDefaultOpusModel: important.claudeDefaultOpusModel,
    codexAuthJsonText,
    codexConfigTomlText,
    openCodeAuthJsonText,
    openClawGatewayUrl: openClawImportant.gatewayUrl,
    openClawGatewayToken: openClawImportant.gatewayToken,
    openClawSessionKey: openClawImportant.sessionKey,
    clineProvider: clineImportant.provider,
    clineApiKey: clineImportant.apiKey,
    clineModel: clineImportant.model,
    clineBaseUrl: clineImportant.baseUrl,
  }
}

function compareVersion(a: string, b: string): number {
  const toParts = (value: string): number[] => {
    const normalized = value.trim().replace(/^[^\d]*/, "")
    return normalized.split(".").map((part) => Number.parseInt(part, 10) || 0)
  }
  const left = toParts(a)
  const right = toParts(b)
  const len = Math.max(left.length, right.length)
  for (let i = 0; i < len; i += 1) {
    const lv = left[i] ?? 0
    const rv = right[i] ?? 0
    if (lv !== rv) return lv > rv ? 1 : -1
  }
  return 0
}

function hasComparableVersion(
  value: string | null | undefined
): value is string {
  return Boolean(value && /\d/.test(value) && value.includes("."))
}

function buildVersionCheck(agent: AcpAgentInfo): UiCheckItem | null {
  if (agent.distribution_type !== "binary" && agent.distribution_type !== "npx")
    return null

  const remoteVersion = agent.registry_version ?? "unknown"
  const localVersion =
    agent.installed_version ?? acpText("version.notInstalled", "Not installed")
  const versionText = acpText(
    "version.remoteLocal",
    "Remote: {remoteVersion} · Local: {localVersion}",
    { remoteVersion, localVersion }
  )
  const installAction: RunningActionKind =
    agent.distribution_type === "binary" ? "download_binary" : "install_npx"
  const upgradeAction: RunningActionKind =
    agent.distribution_type === "binary" ? "upgrade_binary" : "upgrade_npx"
  const uninstallAction: RunningActionKind =
    agent.distribution_type === "binary" ? "uninstall_binary" : "uninstall_npx"

  if (!agent.available) {
    return {
      check_id: "version_status",
      label: acpText("version.statusLabel", "Version Status"),
      status: "fail",
      message: acpText(
        "version.platformUnsupported",
        "{versionText}. Current platform does not support this agent.",
        { versionText }
      ),
      fixes: [],
    }
  }

  if (!agent.installed_version) {
    return {
      check_id: "version_status",
      label: acpText("version.statusLabel", "Version Status"),
      status: "fail",
      message: acpText(
        "version.clickInstall",
        "{versionText}. Click Install on the right.",
        { versionText }
      ),
      fixes: [
        {
          label: acpText("actions.install", "Install"),
          kind: installAction,
          payload: agent.agent_type,
        },
      ],
    }
  }

  if (
    agent.registry_version &&
    hasComparableVersion(agent.registry_version) &&
    !hasComparableVersion(agent.installed_version)
  ) {
    return {
      check_id: "version_status",
      label: acpText("version.statusLabel", "Version Status"),
      status: "warn",
      message: acpText(
        "version.localUnrecognized",
        "{versionText}. Local version is not comparable; try upgrade to overwrite install.",
        { versionText }
      ),
      fixes: [
        {
          label: acpText("actions.upgrade", "Upgrade"),
          kind: upgradeAction,
          payload: agent.agent_type,
        },
        {
          label: acpText("actions.uninstall", "Uninstall"),
          kind: uninstallAction,
          payload: agent.agent_type,
        },
      ],
    }
  }

  if (
    hasComparableVersion(agent.registry_version) &&
    hasComparableVersion(agent.installed_version) &&
    compareVersion(agent.installed_version, agent.registry_version) < 0
  ) {
    return {
      check_id: "version_status",
      label: acpText("version.statusLabel", "Version Status"),
      status: "warn",
      message: acpText(
        "version.upgradeAvailable",
        "{versionText}. Upgrade available.",
        { versionText }
      ),
      fixes: [
        {
          label: acpText("actions.upgrade", "Upgrade"),
          kind: upgradeAction,
          payload: agent.agent_type,
        },
        {
          label: acpText("actions.uninstall", "Uninstall"),
          kind: uninstallAction,
          payload: agent.agent_type,
        },
      ],
    }
  }

  if (!agent.registry_version) {
    return {
      check_id: "version_status",
      label: acpText("version.statusLabel", "Version Status"),
      status: "warn",
      message: acpText(
        "version.remoteUnavailable",
        "{versionText}. Remote version is currently unavailable.",
        { versionText }
      ),
      fixes: [
        {
          label: acpText("actions.uninstall", "Uninstall"),
          kind: uninstallAction,
          payload: agent.agent_type,
        },
      ],
    }
  }

  return {
    check_id: "version_status",
    label: acpText("version.statusLabel", "Version Status"),
    status: "pass",
    message: acpText("version.latest", "{versionText}. Already latest.", {
      versionText,
    }),
    fixes: [
      {
        label: acpText("actions.uninstall", "Uninstall"),
        kind: uninstallAction,
        payload: agent.agent_type,
      },
    ],
  }
}

function getAgentChecks(
  agent: AcpAgentInfo,
  current?: AgentCheckState
): UiCheckItem[] {
  const versionCheck = buildVersionCheck(agent)
  const remoteChecks: UiCheckItem[] = (current?.result?.checks ?? []).map(
    (check) => ({
      ...check,
      fixes: [...check.fixes],
    })
  )
  return versionCheck ? [versionCheck, ...remoteChecks] : remoteChecks
}

interface AgentReorderItemProps {
  agent: AcpAgentInfo
  selected: boolean
  reordering: boolean
  dragging: AgentType | null
  onDragStart: (agentType: AgentType) => void
  onDragEnd: () => void
  onSelect: (agentType: AgentType) => void
  children: (
    startDrag: (event: PointerEvent<HTMLButtonElement>) => void
  ) => ReactNode
}

function AgentReorderItem({
  agent,
  selected,
  reordering,
  dragging,
  onDragStart,
  onDragEnd,
  onSelect,
  children,
}: AgentReorderItemProps) {
  const dragControls = useDragControls()

  const startDrag = useCallback(
    (event: PointerEvent<HTMLButtonElement>) => {
      event.preventDefault()
      event.stopPropagation()
      dragControls.start(event)
    },
    [dragControls]
  )

  return (
    <Reorder.Item
      as="section"
      value={agent}
      data-agent-type={agent.agent_type}
      drag={reordering ? false : "y"}
      dragListener={false}
      dragControls={dragControls}
      dragMomentum={false}
      layout="position"
      className={cn(
        "rounded-lg border bg-card p-3 transition-colors cursor-pointer focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/40",
        selected && "border-primary/60 bg-primary/5",
        dragging === agent.agent_type && "border-primary/60 bg-primary/5"
      )}
      tabIndex={0}
      onDragStart={() => {
        onDragStart(agent.agent_type)
      }}
      onDragEnd={onDragEnd}
      onClick={() => {
        onSelect(agent.agent_type)
      }}
      onKeyDown={(event) => {
        if (event.target !== event.currentTarget) return
        if (event.key !== "Enter" && event.key !== " ") return
        event.preventDefault()
        onSelect(agent.agent_type)
      }}
    >
      {children(startDrag)}
    </Reorder.Item>
  )
}

export function AcpAgentSettings() {
  const locale = useLocale()
  const t = useTranslations("AcpAgentSettings")
  const rawTranslator = t as unknown as AcpTranslator
  acpTranslator = (key, values) => rawTranslator(key, values)
  const searchParams = useSearchParams()
  const [agents, setAgents] = useState<AcpAgentInfo[]>([])
  const [loadingAgents, setLoadingAgents] = useState(true)
  const [loadingError, setLoadingError] = useState<string | null>(null)
  const [checkState, setCheckState] = useState<
    Partial<Record<AgentType, AgentCheckState>>
  >({})
  const [checking, setChecking] = useState<Partial<Record<AgentType, boolean>>>(
    {}
  )
  const [busyBinaryAction, setBusyBinaryAction] = useState<
    Partial<Record<AgentType, boolean>>
  >({})
  const [runningActionKind, setRunningActionKind] = useState<
    Partial<Record<AgentType, RunningActionKind>>
  >({})
  const [savingEnv, setSavingEnv] = useState<
    Partial<Record<AgentType, boolean>>
  >({})
  const [savingConfig, setSavingConfig] = useState<
    Partial<Record<AgentType, boolean>>
  >({})
  const [modelProviders, setModelProviders] = useState<ModelProviderInfo[]>([])
  const [uninstallConfirmAgent, setUninstallConfirmAgent] =
    useState<AcpAgentInfo | null>(null)
  const [expandedChecks, setExpandedChecks] = useState<Record<string, boolean>>(
    {}
  )
  const [selectedAgentType, setSelectedAgentType] = useState<AgentType | null>(
    null
  )
  const [drafts, setDrafts] = useState<Partial<Record<AgentType, AgentDraft>>>(
    {}
  )
  const [configErrors, setConfigErrors] = useState<
    Partial<Record<AgentType, string | null>>
  >({})
  const [showApiKeys, setShowApiKeys] = useState<
    Partial<Record<AgentType, boolean>>
  >({})
  const [openCodeProviderId, setOpenCodeProviderId] = useState("")
  const [openCodeNewProviderId, setOpenCodeNewProviderId] = useState("")
  const [openCodeNewModelIds, setOpenCodeNewModelIds] = useState<
    Record<string, string>
  >({})
  const [openCodeModelIdDrafts, setOpenCodeModelIdDrafts] = useState<
    Record<string, string>
  >({})
  const [openCodeModelConfigExpanded, setOpenCodeModelConfigExpanded] =
    useState<Record<string, boolean>>({})
  const [openCodeDeleteProviderId, setOpenCodeDeleteProviderId] = useState<
    string | null
  >(null)
  const [dragging, setDragging] = useState<AgentType | null>(null)
  const [reordering, setReordering] = useState(false)
  const pendingOrderRef = useRef<AgentType[] | null>(null)
  const busyActionRef = useRef<Set<AgentType>>(new Set())
  const handledSearchAgentRef = useRef<string | null>(null)
  const agentListRef = useRef<HTMLDivElement | null>(null)

  const sortedAgents = useMemo(
    () =>
      [...agents].sort(
        (a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name)
      ),
    [agents]
  )
  const selectedAgent = useMemo(
    () =>
      sortedAgents.find((agent) => agent.agent_type === selectedAgentType) ??
      null,
    [selectedAgentType, sortedAgents]
  )
  const agentTypesKey = useMemo(
    () =>
      [...new Set(agents.map((agent) => agent.agent_type))].sort().join(","),
    [agents]
  )
  const requestedAgentType = useMemo(
    () => searchParams.get("agent"),
    [searchParams]
  )

  const refreshAgents = useCallback(async () => {
    setLoadingAgents(true)
    setLoadingError(null)
    try {
      const [next, providers] = await Promise.all([
        acpListAgents(),
        listModelProviders().catch(() => [] as ModelProviderInfo[]),
      ])
      setAgents(next)
      setModelProviders(providers)
      setDrafts((prev) => {
        const updated = { ...prev }
        for (const agent of next) {
          if (!updated[agent.agent_type]) {
            updated[agent.agent_type] = buildAgentDraft(agent)
          }
        }
        return updated
      })
      setConfigErrors((prev) => {
        const updated = { ...prev }
        for (const agent of next) {
          if (typeof updated[agent.agent_type] !== "undefined") continue
          const configText =
            typeof agent.config_json === "string" ? agent.config_json : ""
          updated[agent.agent_type] = parseConfigJsonText(configText).error
        }
        return updated
      })
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setLoadingError(message)
    } finally {
      setLoadingAgents(false)
    }
  }, [])

  const runPreflight = useCallback(
    async (agentType: AgentType, forceRefresh?: boolean) => {
      setChecking((prev) => ({ ...prev, [agentType]: true }))
      try {
        const [resultState, versionState] = await Promise.allSettled([
          acpPreflight(agentType, forceRefresh),
          acpDetectAgentLocalVersion(agentType),
        ])

        if (versionState.status === "fulfilled") {
          setAgents((prev) => {
            if (versionState.value === null) return prev
            let changed = false
            const next = prev.map((agent) => {
              if (agent.agent_type !== agentType) return agent
              if (agent.installed_version === versionState.value) return agent
              changed = true
              return { ...agent, installed_version: versionState.value }
            })
            return changed ? next : prev
          })
        }

        if (resultState.status === "fulfilled") {
          setCheckState((prev) => ({
            ...prev,
            [agentType]: { result: resultState.value },
          }))
        } else {
          const message =
            resultState.reason instanceof Error
              ? resultState.reason.message
              : String(resultState.reason)
          setCheckState((prev) => ({
            ...prev,
            [agentType]: { error: message },
          }))
        }
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        setCheckState((prev) => ({ ...prev, [agentType]: { error: message } }))
      } finally {
        setChecking((prev) => ({ ...prev, [agentType]: false }))
      }
    },
    []
  )

  const runAllPreflight = useCallback(
    async (agentTypes: AgentType[]) => {
      if (agentTypes.length === 0) return
      setChecking((prev) => {
        const next = { ...prev }
        for (const agentType of agentTypes) {
          next[agentType] = true
        }
        return next
      })
      await Promise.all(agentTypes.map((agentType) => runPreflight(agentType)))
    },
    [runPreflight]
  )

  useEffect(() => {
    refreshAgents().catch((err) => {
      console.error("[Settings] refresh agents failed:", err)
    })
  }, [refreshAgents])

  useEffect(() => {
    if (loadingAgents || !agentTypesKey) return
    const agentTypes = agentTypesKey.split(",") as AgentType[]
    runAllPreflight(agentTypes).catch((err) => {
      console.error("[Settings] run all preflight failed:", err)
    })
  }, [agentTypesKey, loadingAgents, runAllPreflight])

  useEffect(() => {
    if (!requestedAgentType) {
      handledSearchAgentRef.current = null
      return
    }
    if (sortedAgents.length === 0) {
      return
    }
    if (handledSearchAgentRef.current === requestedAgentType) {
      return
    }
    const matched = sortedAgents.find(
      (agent) => agent.agent_type === requestedAgentType
    )
    if (matched) {
      setSelectedAgentType(matched.agent_type)
    }
    handledSearchAgentRef.current = requestedAgentType
  }, [requestedAgentType, sortedAgents])

  useEffect(() => {
    if (!selectedAgentType) return
    const container = agentListRef.current
    if (!container) return
    const selected = container.querySelector<HTMLElement>(
      `[data-agent-type="${selectedAgentType}"]`
    )
    if (!selected) return
    selected.scrollIntoView({ block: "nearest", behavior: "smooth" })
  }, [selectedAgentType, sortedAgents])

  useEffect(() => {
    if (sortedAgents.length === 0) {
      setSelectedAgentType(null)
      return
    }
    setSelectedAgentType((prev) => {
      if (prev && sortedAgents.some((agent) => agent.agent_type === prev)) {
        return prev
      }
      return sortedAgents[0].agent_type
    })
  }, [sortedAgents])

  const persistEnv = useCallback(
    async (
      agentType: AgentType,
      enabled: boolean,
      envText: string,
      modelProviderId?: number | null
    ) => {
      const parsedEnv = parseEnvText(envText)
      setSavingEnv((prev) => ({ ...prev, [agentType]: true }))
      try {
        await acpUpdateAgentEnv(agentType, {
          enabled,
          env: parsedEnv,
          modelProviderId: modelProviderId ?? null,
        })
        setAgents((prev) =>
          prev.map((agent) =>
            agent.agent_type === agentType
              ? {
                  ...agent,
                  enabled,
                  env: parsedEnv,
                  model_provider_id: modelProviderId ?? null,
                }
              : agent
          )
        )
      } finally {
        setSavingEnv((prev) => ({ ...prev, [agentType]: false }))
      }
    },
    []
  )

  const persistConfig = useCallback(
    async (
      agentType: AgentType,
      configText: string,
      options?: {
        openCodeAuthJsonText?: string
        codexAuthJsonText?: string
        codexConfigTomlText?: string
      }
    ) => {
      const parsedConfig = parseConfigJsonText(configText)
      if (parsedConfig.error) {
        throw new Error(parsedConfig.error)
      }
      const codexAuthJsonText = options?.codexAuthJsonText
      if (agentType === "codex" && typeof codexAuthJsonText === "string") {
        const authError = parseCodexAuthJsonText(codexAuthJsonText)
        if (authError) {
          throw new Error(authError)
        }
      }
      const normalizedConfig = normalizeConfigText(configText)
      // For agents using merge strategy, mark removed keys as null
      // so the backend merge_json_values can delete them from disk.
      let configForPersist =
        agentType === "open_code" && !normalizedConfig ? "{}" : normalizedConfig
      const usesMerge =
        agentType === "claude_code" ||
        agentType === "gemini" ||
        agentType === "open_claw"
      if (usesMerge && configForPersist) {
        const originalAgent = agents.find((a) => a.agent_type === agentType)
        const originalConfig = originalAgent?.config_json
          ? parseConfigJsonText(originalAgent.config_json).config
          : {}
        const currentConfig = parsedConfig.config
        configForPersist = JSON.stringify(
          markRemovedKeysNull(originalConfig, currentConfig),
          null,
          2
        )
      }
      setSavingConfig((prev) => ({ ...prev, [agentType]: true }))
      try {
        await acpUpdateAgentConfig(agentType, {
          config_json: configForPersist || null,
          opencode_auth_json:
            typeof options?.openCodeAuthJsonText === "string"
              ? options.openCodeAuthJsonText
              : null,
          codex_auth_json:
            typeof codexAuthJsonText === "string" ? codexAuthJsonText : null,
          codex_config_toml:
            typeof options?.codexConfigTomlText === "string"
              ? options.codexConfigTomlText
              : null,
        })
        setAgents((prev) =>
          prev.map((agent) =>
            agent.agent_type === agentType
              ? {
                  ...agent,
                  config_json: normalizedConfig || null,
                  opencode_auth_json:
                    typeof options?.openCodeAuthJsonText === "string"
                      ? options.openCodeAuthJsonText
                      : agent.opencode_auth_json,
                  codex_auth_json:
                    typeof codexAuthJsonText === "string"
                      ? codexAuthJsonText
                      : agent.codex_auth_json,
                  codex_config_toml:
                    typeof options?.codexConfigTomlText === "string"
                      ? options.codexConfigTomlText
                      : agent.codex_config_toml,
                }
              : agent
          )
        )
      } finally {
        setSavingConfig((prev) => ({ ...prev, [agentType]: false }))
      }
    },
    [agents]
  )

  const runBinaryAction = useCallback(
    async (
      agent: AcpAgentInfo,
      mode: "download" | "upgrade",
      kind?: RunningActionKind
    ) => {
      if (busyActionRef.current.has(agent.agent_type)) return
      busyActionRef.current.add(agent.agent_type)
      setBusyBinaryAction((prev) => ({ ...prev, [agent.agent_type]: true }))
      setRunningActionKind((prev) => ({
        ...prev,
        [agent.agent_type]:
          kind ?? (mode === "download" ? "download_binary" : "upgrade_binary"),
      }))
      try {
        if (mode === "upgrade") {
          await acpClearBinaryCache(agent.agent_type)
        }
        await acpDownloadAgentBinary(agent.agent_type)
        await runPreflight(agent.agent_type)
        const detectedVersion = await acpDetectAgentLocalVersion(
          agent.agent_type
        )
        setAgents((prev) =>
          prev.map((item) =>
            item.agent_type === agent.agent_type
              ? { ...item, installed_version: detectedVersion }
              : item
          )
        )
        toast.success(
          t("toasts.agentActionCompleted", {
            name: agent.name,
            action:
              mode === "upgrade" ? t("actions.upgrade") : t("actions.install"),
          }),
          {
            description: detectedVersion
              ? t("toasts.localVersion", { version: detectedVersion })
              : t("toasts.installCompletedVersionLater"),
          }
        )
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        toast.error(
          t("toasts.agentActionFailed", {
            name: agent.name,
            action:
              mode === "upgrade" ? t("actions.upgrade") : t("actions.install"),
          }),
          {
            description: message,
          }
        )
        throw err
      } finally {
        busyActionRef.current.delete(agent.agent_type)
        setBusyBinaryAction((prev) => ({ ...prev, [agent.agent_type]: false }))
        setRunningActionKind((prev) => ({
          ...prev,
          [agent.agent_type]: undefined,
        }))
      }
    },
    [runPreflight, t]
  )

  const runNpxAction = useCallback(
    async (agent: AcpAgentInfo, mode: "install" | "upgrade") => {
      if (busyActionRef.current.has(agent.agent_type)) return
      busyActionRef.current.add(agent.agent_type)
      setBusyBinaryAction((prev) => ({ ...prev, [agent.agent_type]: true }))
      setRunningActionKind((prev) => ({
        ...prev,
        [agent.agent_type]: mode === "install" ? "install_npx" : "upgrade_npx",
      }))
      try {
        const installedVersion = await acpPrepareNpxAgent(
          agent.agent_type,
          agent.registry_version
        )
        setAgents((prev) =>
          prev.map((item) =>
            item.agent_type === agent.agent_type
              ? { ...item, installed_version: installedVersion }
              : item
          )
        )
        await runPreflight(agent.agent_type)
        const detectedVersion = await acpDetectAgentLocalVersion(
          agent.agent_type
        )
        if (detectedVersion && detectedVersion !== installedVersion) {
          setAgents((prev) =>
            prev.map((item) =>
              item.agent_type === agent.agent_type
                ? { ...item, installed_version: detectedVersion }
                : item
            )
          )
        }
        const finalVersion = detectedVersion ?? installedVersion
        toast.success(
          t("toasts.agentActionCompleted", {
            name: agent.name,
            action:
              mode === "upgrade" ? t("actions.upgrade") : t("actions.install"),
          }),
          {
            description: finalVersion
              ? t("toasts.localVersion", { version: finalVersion })
              : t("toasts.installCompletedVersionLater"),
          }
        )
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        toast.error(
          t("toasts.agentActionFailed", {
            name: agent.name,
            action:
              mode === "upgrade" ? t("actions.upgrade") : t("actions.install"),
          }),
          {
            description: message,
          }
        )
        throw err
      } finally {
        busyActionRef.current.delete(agent.agent_type)
        setBusyBinaryAction((prev) => ({ ...prev, [agent.agent_type]: false }))
        setRunningActionKind((prev) => ({
          ...prev,
          [agent.agent_type]: undefined,
        }))
      }
    },
    [runPreflight, t]
  )

  const runUninstallAction = useCallback(
    async (agent: AcpAgentInfo) => {
      if (busyActionRef.current.has(agent.agent_type)) return
      busyActionRef.current.add(agent.agent_type)
      setBusyBinaryAction((prev) => ({ ...prev, [agent.agent_type]: true }))
      setRunningActionKind((prev) => ({
        ...prev,
        [agent.agent_type]:
          agent.distribution_type === "binary"
            ? "uninstall_binary"
            : "uninstall_npx",
      }))
      try {
        await acpUninstallAgent(agent.agent_type)
        setAgents((prev) =>
          prev.map((item) =>
            item.agent_type === agent.agent_type
              ? { ...item, installed_version: null }
              : item
          )
        )
        await runPreflight(agent.agent_type)
        toast.success(t("toasts.uninstallCompleted", { name: agent.name }), {
          description: t("toasts.localVersionRemoved"),
        })
      } catch (err) {
        const message = err instanceof Error ? err.message : String(err)
        toast.error(t("toasts.uninstallFailed", { name: agent.name }), {
          description: message,
        })
        throw err
      } finally {
        busyActionRef.current.delete(agent.agent_type)
        setBusyBinaryAction((prev) => ({ ...prev, [agent.agent_type]: false }))
        setRunningActionKind((prev) => ({
          ...prev,
          [agent.agent_type]: undefined,
        }))
      }
    },
    [runPreflight, t]
  )

  const handleFixAction = async (agent: AcpAgentInfo, action: UiFixAction) => {
    if (
      busyBinaryAction[agent.agent_type] ||
      busyActionRef.current.has(agent.agent_type)
    ) {
      return
    }
    if (action.kind === "open_url") {
      await openUrl(action.payload)
      return
    }
    if (action.kind === "download_binary") {
      await runBinaryAction(agent, "download")
      return
    }
    if (action.kind === "upgrade_binary") {
      await runBinaryAction(agent, "upgrade")
      return
    }
    if (action.kind === "install_npx") {
      await runNpxAction(agent, "install")
      return
    }
    if (action.kind === "upgrade_npx") {
      await runNpxAction(agent, "upgrade")
      return
    }
    if (action.kind === "uninstall_binary" || action.kind === "uninstall_npx") {
      setUninstallConfirmAgent(agent)
      return
    }
    if (action.kind === "redownload_binary") {
      await runBinaryAction(agent, "upgrade", "redownload_binary")
      return
    }
    await runPreflight(agent.agent_type)
  }

  const confirmUninstall = useCallback(() => {
    if (!uninstallConfirmAgent) return
    const target = uninstallConfirmAgent
    runUninstallAction(target)
      .catch((err) => {
        console.error("[Settings] uninstall action failed:", err)
      })
      .finally(() => {
        setUninstallConfirmAgent(null)
      })
  }, [runUninstallAction, uninstallConfirmAgent])

  const persistReorder = useCallback(
    async (order: AgentType[]) => {
      if (order.length === 0) return
      setReordering(true)
      try {
        await acpReorderAgents(order)
      } catch (err) {
        console.error("[Settings] reorder agents failed:", err)
        const message = err instanceof Error ? err.message : String(err)
        toast.error(t("toasts.saveAgentOrderFailed"), {
          description: message,
        })
        await refreshAgents()
      } finally {
        setReordering(false)
      }
    },
    [refreshAgents, t]
  )

  const handleReorder = useCallback((next: AcpAgentInfo[]) => {
    const reordered = next.map((agent, index) => ({
      ...agent,
      sort_order: index,
    }))
    setAgents(reordered)
    pendingOrderRef.current = reordered.map((agent) => agent.agent_type)
  }, [])

  const renderCheck = (agent: AcpAgentInfo, check: UiCheckItem) => {
    const checkKey = `${agent.agent_type}:${check.check_id}`
    const expanded = expandedChecks[checkKey] ?? check.status !== "pass"

    return (
      <div
        key={check.check_id}
        className="rounded-md border bg-muted/20 px-3 py-2 space-y-2"
      >
        <button
          type="button"
          className="w-full flex items-center justify-between gap-2 text-left"
          onClick={() => {
            setExpandedChecks((prev) => ({
              ...prev,
              [checkKey]: !expanded,
            }))
          }}
        >
          <div className="min-w-0 flex items-center gap-1.5">
            {expanded ? (
              <ChevronDown className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
            ) : (
              <ChevronRight className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
            )}
            <span className="text-xs font-medium truncate">{check.label}</span>
          </div>
          <span
            className={`text-[11px] font-semibold shrink-0 ${statusTone(check.status)}`}
          >
            {check.status.toUpperCase()}
          </span>
        </button>

        {expanded && (
          <div className="flex items-start justify-between gap-2">
            <div className="min-w-0 text-[11px] text-muted-foreground break-words">
              {check.message}
            </div>
            {check.fixes.length > 0 && (
              <div className="flex flex-wrap gap-1.5 justify-end max-w-[220px] shrink-0">
                {check.fixes.map((fix, index) => (
                  <Button
                    key={`${fix.label}-${index}`}
                    size="xs"
                    variant="outline"
                    className="h-6 bg-muted/30 hover:bg-muted/50 disabled:bg-muted/30 disabled:opacity-100"
                    disabled={
                      Boolean(busyBinaryAction[agent.agent_type]) &&
                      [
                        "download_binary",
                        "upgrade_binary",
                        "install_npx",
                        "upgrade_npx",
                        "uninstall_binary",
                        "uninstall_npx",
                        "redownload_binary",
                      ].includes(fix.kind)
                    }
                    onClick={() => {
                      handleFixAction(agent, fix).catch((err) => {
                        console.error("[Settings] fix action failed:", err)
                      })
                    }}
                  >
                    {runningActionKind[agent.agent_type] === fix.kind ? (
                      <Loader2 className="h-3 w-3 animate-spin" />
                    ) : fix.kind === "download_binary" ||
                      fix.kind === "install_npx" ? (
                      <Download className="h-3 w-3" />
                    ) : fix.kind === "upgrade_binary" ||
                      fix.kind === "upgrade_npx" ||
                      fix.kind === "redownload_binary" ? (
                      <Wrench className="h-3 w-3" />
                    ) : fix.kind === "uninstall_binary" ||
                      fix.kind === "uninstall_npx" ? (
                      <Trash2 className="h-3 w-3" />
                    ) : null}
                    {fix.label}
                  </Button>
                ))}
              </div>
            )}
          </div>
        )}
      </div>
    )
  }

  const selectedCurrent = selectedAgent
    ? checkState[selectedAgent.agent_type]
    : undefined
  const selectedDraft = selectedAgent
    ? (drafts[selectedAgent.agent_type] ?? buildAgentDraft(selectedAgent))
    : null
  const selectedConfigError = selectedAgent
    ? (configErrors[selectedAgent.agent_type] ?? null)
    : null
  const selectedIsSaving = selectedAgent
    ? Boolean(
        savingEnv[selectedAgent.agent_type] ||
        savingConfig[selectedAgent.agent_type]
      )
    : false
  const selectedIsSavingEnv = selectedAgent
    ? Boolean(savingEnv[selectedAgent.agent_type])
    : false
  const selectedIsSavingConfig = selectedAgent
    ? Boolean(savingConfig[selectedAgent.agent_type])
    : false
  const selectedAgentKind = selectedAgent?.agent_type ?? null

  const selectedModelProviders = useMemo(() => {
    if (!selectedAgent) return []
    return modelProviders.filter((p) =>
      p.agent_types.includes(selectedAgent.agent_type)
    )
  }, [modelProviders, selectedAgent])

  const selectedNeedsModelProvider = useMemo(() => {
    if (!selectedDraft) return false
    if (!selectedAgent) return false
    const at = selectedAgent.agent_type
    if (at === "claude_code")
      return selectedDraft.claudeAuthMode === "model_provider"
    if (at === "codex") return selectedDraft.codexAuthMode === "model_provider"
    if (at === "gemini")
      return selectedDraft.geminiAuthMode === "model_provider"
    return false
  }, [selectedAgent, selectedDraft])

  const selectedMissingModelProvider =
    selectedNeedsModelProvider && selectedDraft?.modelProviderId == null
  const selectedCodexAuthJsonText = selectedDraft?.codexAuthJsonText ?? ""
  const selectedConfigText = selectedDraft?.configText ?? ""
  const selectedOpenCodeAuthJsonText = selectedDraft?.openCodeAuthJsonText ?? ""
  const selectedCodexAuthError = useMemo(() => {
    if (selectedAgentKind !== "codex" || !locale) return null
    return parseCodexAuthJsonText(selectedCodexAuthJsonText)
  }, [locale, selectedAgentKind, selectedCodexAuthJsonText])
  const selectedCodexReasoningEffortOption =
    selectedAgent?.agent_type === "codex" && selectedDraft
      ? (CODEX_REASONING_EFFORT_OPTIONS.find(
          (option) => option.value === selectedDraft.codexReasoningEffort
        ) ?? null)
      : null
  const selectedOpenCodeConfig = useMemo(() => {
    if (selectedAgentKind !== "open_code" || !locale) return null
    return extractOpenCodeConfigValues(
      selectedConfigText,
      selectedOpenCodeAuthJsonText
    )
  }, [
    locale,
    selectedAgentKind,
    selectedConfigText,
    selectedOpenCodeAuthJsonText,
  ])
  const openCodeModelOptions = useMemo(
    () => buildOpenCodeModelOptions(selectedOpenCodeConfig),
    [selectedOpenCodeConfig]
  )
  const selectedChecks = useMemo(() => {
    if (!selectedAgent || !locale) return []
    return getAgentChecks(selectedAgent, selectedCurrent)
  }, [locale, selectedAgent, selectedCurrent])

  useEffect(() => {
    if (!selectedAgent || selectedChecks.length === 0) return
    setExpandedChecks((prev) => {
      let next = prev
      for (const check of selectedChecks) {
        const key = `${selectedAgent.agent_type}:${check.check_id}`
        if (typeof next[key] !== "undefined") continue
        if (next === prev) next = { ...prev }
        next[key] = check.status !== "pass"
      }
      return next
    })
  }, [selectedAgent, selectedChecks])

  useEffect(() => {
    if (!selectedOpenCodeConfig) {
      if (openCodeProviderId) setOpenCodeProviderId("")
      return
    }
    if (!openCodeProviderId) return
    if (selectedOpenCodeConfig.providerIds.includes(openCodeProviderId)) {
      return
    }
    setOpenCodeProviderId("")
  }, [openCodeProviderId, selectedOpenCodeConfig])

  useEffect(() => {
    if (!openCodeDeleteProviderId) return
    if (!selectedOpenCodeConfig) {
      setOpenCodeDeleteProviderId(null)
      return
    }
    if (
      !selectedOpenCodeConfig.providerIds.includes(openCodeDeleteProviderId)
    ) {
      setOpenCodeDeleteProviderId(null)
    }
  }, [openCodeDeleteProviderId, selectedOpenCodeConfig])

  const updateSelectedDraft = useCallback(
    (updater: (current: AgentDraft) => AgentDraft) => {
      if (!selectedAgent || !selectedDraft) return
      setDrafts((prev) => {
        const current = prev[selectedAgent.agent_type] ?? selectedDraft
        return {
          ...prev,
          [selectedAgent.agent_type]: updater(current),
        }
      })
    },
    [selectedAgent, selectedDraft]
  )

  const handleConfigTextChange = useCallback(
    (nextText: string) => {
      if (!selectedAgent || !selectedDraft) return
      const parseResult = parseConfigJsonText(nextText)
      setConfigErrors((prev) => ({
        ...prev,
        [selectedAgent.agent_type]: parseResult.error,
      }))

      if (parseResult.error) {
        updateSelectedDraft((current) => ({
          ...current,
          configText: nextText,
        }))
        return
      }

      if (selectedAgent.agent_type === "open_code") {
        const openCode = extractOpenCodeConfigValues(
          nextText,
          selectedDraft.openCodeAuthJsonText
        )
        updateSelectedDraft((current) => ({
          ...current,
          configText: nextText,
          model: openCode.model,
        }))
        return
      }

      if (selectedAgent.agent_type === "cline") {
        const cline = extractClineImportantValues(nextText)
        updateSelectedDraft((current) => ({
          ...current,
          configText: nextText,
          clineProvider: cline.provider,
          clineApiKey: cline.apiKey,
          clineModel: cline.model,
          clineBaseUrl: cline.baseUrl,
        }))
        return
      }

      const important = extractImportantConfigValues(
        selectedAgent.agent_type,
        parseEnvText(selectedDraft.envText),
        nextText
      )
      const geminiImportant =
        selectedAgent.agent_type === "gemini"
          ? extractGeminiImportantValues(
              parseEnvText(selectedDraft.envText),
              nextText
            )
          : null
      updateSelectedDraft((current) => ({
        ...current,
        configText: nextText,
        apiBaseUrl: geminiImportant
          ? geminiImportant.apiBaseUrl
          : important.apiBaseUrl,
        apiKey: important.apiKey,
        model: geminiImportant ? geminiImportant.model : important.model,
        geminiAuthMode: geminiImportant
          ? geminiImportant.authMode
          : current.geminiAuthMode,
        geminiApiKey: geminiImportant
          ? geminiImportant.geminiApiKey
          : current.geminiApiKey,
        googleApiKey: geminiImportant
          ? geminiImportant.googleApiKey
          : current.googleApiKey,
        googleCloudProject: geminiImportant
          ? geminiImportant.googleCloudProject
          : current.googleCloudProject,
        googleCloudLocation: geminiImportant
          ? geminiImportant.googleCloudLocation
          : current.googleCloudLocation,
        googleApplicationCredentials: geminiImportant
          ? geminiImportant.googleApplicationCredentials
          : current.googleApplicationCredentials,
        claudeMainModel: important.claudeMainModel,
        claudeReasoningModel: important.claudeReasoningModel,
        claudeDefaultHaikuModel: important.claudeDefaultHaikuModel,
        claudeDefaultSonnetModel: important.claudeDefaultSonnetModel,
        claudeDefaultOpusModel: important.claudeDefaultOpusModel,
      }))
    },
    [selectedAgent, selectedDraft, updateSelectedDraft]
  )

  const handleImportantConfigChange = useCallback(
    (key: ImportantConfigKey, value: string) => {
      if (!selectedAgent || !selectedDraft) return
      const nextDraft = applyImportantFieldToDraft(selectedDraft, key, value)
      const nextJson = patchImportantConfigText(
        selectedAgent.agent_type,
        selectedDraft.configText,
        buildImportantPatchFromDraft(nextDraft)
      )
      if (nextJson.recoveredFromInvalid) {
        toast.warning(t("warnings.nativeJsonRecoveredStructured"))
      }
      setConfigErrors((prev) => ({
        ...prev,
        [selectedAgent.agent_type]: null,
      }))
      updateSelectedDraft((current) => {
        const nextCurrent = applyImportantFieldToDraft(current, key, value)
        return {
          ...nextCurrent,
          envText: patchEnvByImportantKey(
            selectedAgent.agent_type,
            current.envText,
            key,
            value
          ),
          configText: nextJson.configText,
        }
      })
    },
    [selectedAgent, selectedDraft, t, updateSelectedDraft]
  )

  const handleClaudeAuthModeChange = useCallback(
    (nextMode: ClaudeAuthMode) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "claude_code"
      )
        return

      const keys = importantEnvKeysByAgent("claude_code")
      const allEnvKeys = [...keys.apiBaseUrl, ...keys.apiKey]

      if (nextMode === "official_subscription") {
        // Clear API URL/API Key from env and config
        const envPatch: Record<string, string> = {}
        for (const k of allEnvKeys) envPatch[k] = ""
        // Build clean display config (remove null keys)
        const parsed = parseConfigJsonText(selectedDraft.configText)
        const config: Record<string, unknown> = parsed.error
          ? {}
          : { ...parsed.config }
        delete config.apiBaseUrl
        delete config.apiKey
        if (config.env && typeof config.env === "object") {
          const cfgEnv = { ...(config.env as Record<string, unknown>) }
          for (const k of allEnvKeys) delete cfgEnv[k]
          if (Object.keys(cfgEnv).length > 0) {
            config.env = cfgEnv
          } else {
            delete config.env
          }
        }
        const nextConfigText =
          Object.keys(config).length > 0 ? JSON.stringify(config, null, 2) : ""
        setConfigErrors((prev) => ({
          ...prev,
          [selectedAgent.agent_type]: null,
        }))
        updateSelectedDraft((current) => ({
          ...current,
          claudeAuthMode: nextMode,
          modelProviderId: null,
          apiBaseUrl: "",
          apiKey: "",
          envText: patchEnvText(current.envText, envPatch),
          configText: nextConfigText,
        }))
        return
      }

      // "custom" or "model_provider" — keep existing values, just switch mode
      updateSelectedDraft((current) => ({
        ...current,
        claudeAuthMode: nextMode,
        modelProviderId:
          nextMode === "model_provider" ? current.modelProviderId : null,
      }))
    },
    [selectedAgent, selectedDraft, updateSelectedDraft]
  )

  const handleModelProviderSelect = useCallback(
    (providerIdStr: string) => {
      if (!selectedAgent || !selectedDraft) return
      const providerId = providerIdStr ? Number(providerIdStr) : null
      const provider = providerId
        ? modelProviders.find((p) => p.id === providerId)
        : null
      const apiUrl = provider?.api_url ?? ""
      const apiKey = provider?.api_key ?? ""
      const agentType = selectedAgent.agent_type

      if (agentType === "claude_code") {
        const nextConfigJson = patchImportantConfigText(
          agentType,
          selectedDraft.configText,
          { apiBaseUrl: apiUrl, apiKey }
        )
        setConfigErrors((prev) => ({
          ...prev,
          [agentType]: null,
        }))
        updateSelectedDraft((current) => ({
          ...current,
          modelProviderId: providerId,
          apiBaseUrl: apiUrl,
          apiKey,
          envText: patchEnvByImportantKey(
            agentType,
            patchEnvByImportantKey(
              agentType,
              current.envText,
              "apiBaseUrl",
              apiUrl
            ),
            "apiKey",
            apiKey
          ),
          configText: nextConfigJson.configText,
        }))
      } else if (agentType === "codex") {
        const nextAuthPatch = patchCodexAuthJsonText(
          selectedDraft.codexAuthJsonText,
          { apiKey, authMode: null }
        )
        const nextAuthJsonText = nextAuthPatch.authJsonText
        const nextConfigTomlText = patchCodexConfigTomlText(
          selectedDraft.codexConfigTomlText,
          {
            modelProvider: CODEX_DEFAULT_MODEL_PROVIDER,
            apiBaseUrl: apiUrl,
          }
        )
        const synced = extractCodexImportantValues(
          nextAuthJsonText,
          nextConfigTomlText
        )
        updateSelectedDraft((current) => ({
          ...current,
          modelProviderId: providerId,
          apiBaseUrl: apiUrl,
          apiKey,
          codexAuthJsonText: nextAuthJsonText,
          codexConfigTomlText: nextConfigTomlText,
          codexModelProvider: CODEX_DEFAULT_MODEL_PROVIDER,
          codexProviderOptions: synced.providerOptions,
          envText: patchEnvText(current.envText, {
            OPENAI_API_KEY: apiKey,
            OPENAI_BASE_URL: apiUrl,
          }),
        }))
      } else if (agentType === "gemini") {
        const nextConfigJson = patchGeminiConfigText(selectedDraft.configText, {
          apiBaseUrl: apiUrl,
          geminiApiKey: apiKey,
        })
        setConfigErrors((prev) => ({
          ...prev,
          [agentType]: null,
        }))
        updateSelectedDraft((current) => ({
          ...current,
          modelProviderId: providerId,
          apiBaseUrl: apiUrl,
          apiKey,
          geminiApiKey: apiKey,
          envText: patchGeminiEnvText(current.envText, {
            apiBaseUrl: apiUrl,
            geminiApiKey: apiKey,
          }),
          configText: nextConfigJson.configText,
        }))
      } else {
        updateSelectedDraft((current) => ({
          ...current,
          modelProviderId: providerId,
        }))
      }
    },
    [selectedAgent, selectedDraft, modelProviders, updateSelectedDraft]
  )

  const handleGeminiFieldChange = useCallback(
    (
      key:
        | "apiBaseUrl"
        | "model"
        | "geminiApiKey"
        | "googleApiKey"
        | "googleCloudProject"
        | "googleCloudLocation"
        | "googleApplicationCredentials",
      value: string
    ) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "gemini"
      )
        return

      const nextValues = {
        authMode: selectedDraft.geminiAuthMode,
        apiBaseUrl: selectedDraft.apiBaseUrl,
        geminiApiKey: selectedDraft.geminiApiKey,
        googleApiKey: selectedDraft.googleApiKey,
        googleCloudProject: selectedDraft.googleCloudProject,
        googleCloudLocation: selectedDraft.googleCloudLocation,
        googleApplicationCredentials:
          selectedDraft.googleApplicationCredentials,
        model: selectedDraft.model,
      }
      nextValues[key] = value
      const normalizedValues = patchGeminiAuthMode(
        nextValues,
        nextValues.authMode
      )

      const nextConfig = patchGeminiConfigText(selectedDraft.configText, {
        apiBaseUrl: normalizedValues.apiBaseUrl,
        model: normalizedValues.model,
        geminiApiKey: normalizedValues.geminiApiKey,
        googleApiKey: normalizedValues.googleApiKey,
        googleCloudProject: normalizedValues.googleCloudProject,
        googleCloudLocation: normalizedValues.googleCloudLocation,
        googleApplicationCredentials:
          normalizedValues.googleApplicationCredentials,
      })
      if (nextConfig.recoveredFromInvalid) {
        toast.warning(t("warnings.nativeJsonRecoveredStructured"))
      }
      setConfigErrors((prev) => ({
        ...prev,
        [selectedAgent.agent_type]: null,
      }))

      updateSelectedDraft((current) => {
        const nextEnvText = patchGeminiEnvText(current.envText, {
          apiBaseUrl: normalizedValues.apiBaseUrl,
          model: normalizedValues.model,
          geminiApiKey: normalizedValues.geminiApiKey,
          googleApiKey: normalizedValues.googleApiKey,
          googleCloudProject: normalizedValues.googleCloudProject,
          googleCloudLocation: normalizedValues.googleCloudLocation,
          googleApplicationCredentials:
            normalizedValues.googleApplicationCredentials,
        })
        return {
          ...current,
          apiBaseUrl: normalizedValues.apiBaseUrl,
          model: normalizedValues.model,
          apiKey:
            normalizedValues.geminiApiKey || normalizedValues.googleApiKey,
          geminiAuthMode: normalizedValues.authMode,
          geminiApiKey: normalizedValues.geminiApiKey,
          googleApiKey: normalizedValues.googleApiKey,
          googleCloudProject: normalizedValues.googleCloudProject,
          googleCloudLocation: normalizedValues.googleCloudLocation,
          googleApplicationCredentials:
            normalizedValues.googleApplicationCredentials,
          envText: nextEnvText,
          configText: nextConfig.configText,
        }
      })
    },
    [selectedAgent, selectedDraft, t, updateSelectedDraft]
  )

  const handleGeminiAuthModeChange = useCallback(
    (nextMode: GeminiAuthMode) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "gemini"
      )
        return

      if (nextMode === "model_provider") {
        // Keep existing values; provider selection will fill API URL/Key
        updateSelectedDraft((current) => ({
          ...current,
          geminiAuthMode: nextMode,
          modelProviderId: current.modelProviderId,
        }))
        return
      }

      const patched = patchGeminiAuthMode(
        {
          authMode: selectedDraft.geminiAuthMode,
          apiBaseUrl: selectedDraft.apiBaseUrl,
          geminiApiKey: selectedDraft.geminiApiKey,
          googleApiKey: selectedDraft.googleApiKey,
          googleCloudProject: selectedDraft.googleCloudProject,
          googleCloudLocation: selectedDraft.googleCloudLocation,
          googleApplicationCredentials:
            selectedDraft.googleApplicationCredentials,
          model: selectedDraft.model,
        },
        nextMode
      )

      const nextConfig = patchGeminiConfigText(selectedDraft.configText, {
        apiBaseUrl: patched.apiBaseUrl,
        model: patched.model,
        geminiApiKey: patched.geminiApiKey,
        googleApiKey: patched.googleApiKey,
        googleCloudProject: patched.googleCloudProject,
        googleCloudLocation: patched.googleCloudLocation,
        googleApplicationCredentials: patched.googleApplicationCredentials,
      })
      if (nextConfig.recoveredFromInvalid) {
        toast.warning(t("warnings.nativeJsonRecoveredStructured"))
      }
      setConfigErrors((prev) => ({
        ...prev,
        [selectedAgent.agent_type]: null,
      }))

      updateSelectedDraft((current) => ({
        ...current,
        geminiAuthMode: patched.authMode,
        modelProviderId: null,
        apiBaseUrl: patched.apiBaseUrl,
        apiKey: patched.geminiApiKey || patched.googleApiKey,
        geminiApiKey: patched.geminiApiKey,
        googleApiKey: patched.googleApiKey,
        googleCloudProject: patched.googleCloudProject,
        googleCloudLocation: patched.googleCloudLocation,
        googleApplicationCredentials: patched.googleApplicationCredentials,
        envText: patchGeminiEnvText(current.envText, {
          apiBaseUrl: patched.apiBaseUrl,
          model: patched.model,
          geminiApiKey: patched.geminiApiKey,
          googleApiKey: patched.googleApiKey,
          googleCloudProject: patched.googleCloudProject,
          googleCloudLocation: patched.googleCloudLocation,
          googleApplicationCredentials: patched.googleApplicationCredentials,
        }),
        configText: nextConfig.configText,
      }))
    },
    [selectedAgent, selectedDraft, t, updateSelectedDraft]
  )

  const handleOpenClawFieldChange = useCallback(
    (
      key: "openClawGatewayUrl" | "openClawGatewayToken" | "openClawSessionKey",
      value: string
    ) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "open_claw"
      )
        return

      const envKeyMap: Record<string, string> = {
        openClawGatewayUrl: OPENCLAW_ENV_KEYS.gatewayUrl,
        openClawGatewayToken: OPENCLAW_ENV_KEYS.gatewayToken,
        openClawSessionKey: OPENCLAW_ENV_KEYS.sessionKey,
      }

      updateSelectedDraft((current) => ({
        ...current,
        [key]: value,
        envText: patchEnvText(current.envText, {
          [envKeyMap[key]]: value,
        }),
      }))
    },
    [selectedAgent, selectedDraft, updateSelectedDraft]
  )

  const handleClineFieldChange = useCallback(
    (
      key: "clineProvider" | "clineApiKey" | "clineModel" | "clineBaseUrl",
      value: string
    ) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "cline"
      )
        return

      updateSelectedDraft((current) => {
        const next = { ...current, [key]: value }
        // Rebuild config_json from Cline draft fields
        const config: Record<string, unknown> = {}
        config.apiProvider =
          key === "clineProvider" ? value : next.clineProvider
        const apiKey = key === "clineApiKey" ? value : next.clineApiKey
        if (apiKey.trim()) config.apiKey = apiKey.trim()
        const model = key === "clineModel" ? value : next.clineModel
        if (model.trim()) config.model = model.trim()
        const baseUrl = key === "clineBaseUrl" ? value : next.clineBaseUrl
        if (baseUrl.trim()) config.apiBaseUrl = baseUrl.trim()
        next.configText = JSON.stringify(config, null, 2)
        return next
      })
    },
    [selectedAgent, selectedDraft, updateSelectedDraft]
  )

  const handleOpenCodeConfigPatch = useCallback(
    (mutator: (config: Record<string, unknown>) => void) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "open_code"
      )
        return
      const nextConfig = patchOpenCodeConfigText(
        selectedDraft.configText,
        mutator
      )
      if (nextConfig.recoveredFromInvalid) {
        toast.warning(t("warnings.nativeJsonRecoveredOpenCode"))
      }
      setConfigErrors((prev) => ({
        ...prev,
        [selectedAgent.agent_type]: null,
      }))
      const parsed = extractOpenCodeConfigValues(
        nextConfig.configText,
        selectedDraft.openCodeAuthJsonText
      )
      updateSelectedDraft((current) => ({
        ...current,
        configText: nextConfig.configText,
        model: parsed.model,
      }))
    },
    [selectedAgent, selectedDraft, t, updateSelectedDraft]
  )

  const handleOpenCodeFieldChange = useCallback(
    (key: "model" | "small_model", value: string) => {
      handleOpenCodeConfigPatch((config) => {
        const trimmed = value.trim()
        if (!trimmed) {
          delete config[key]
          return
        }
        config[key] = trimmed
      })
    },
    [handleOpenCodeConfigPatch]
  )

  const handleOpenCodeAddProvider = useCallback(() => {
    if (!selectedOpenCodeConfig) return
    const providerId = openCodeNewProviderId.trim()
    if (!providerId) return
    if (!/^[A-Za-z0-9_.-]+$/.test(providerId)) {
      toast.error(t("errors.providerIdPattern"))
      return
    }
    if (selectedOpenCodeConfig.providerIds.includes(providerId)) {
      toast.error(t("errors.providerExists", { providerId }))
      return
    }
    handleOpenCodeConfigPatch((config) => {
      const providerRoot = asObjectRecord(config.provider) ?? {}
      if (!asObjectRecord(config.provider)) {
        config.provider = providerRoot
      }
      providerRoot[providerId] = {
        options: {},
        models: {},
      }
    })
    setOpenCodeProviderId(providerId)
    setOpenCodeNewProviderId("")
  }, [
    handleOpenCodeConfigPatch,
    openCodeNewProviderId,
    selectedOpenCodeConfig,
    t,
  ])

  const handleOpenCodeRemoveProvider = useCallback(
    (providerId: string) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "open_code"
      ) {
        return null
      }
      const targetId = providerId.trim()
      if (!targetId) return null

      const nextConfig = patchOpenCodeConfigText(
        selectedDraft.configText,
        (config) => {
          const providerRoot = asObjectRecord(config.provider)
          if (providerRoot) {
            delete providerRoot[targetId]
            if (Object.keys(providerRoot).length === 0) {
              delete config.provider
            }
          }

          const enabledProviders = Array.isArray(config.enabled_providers)
            ? config.enabled_providers
                .filter((item): item is string => typeof item === "string")
                .filter((item) => item !== targetId)
            : []
          if (enabledProviders.length > 0) {
            config.enabled_providers = enabledProviders
          } else {
            delete config.enabled_providers
          }

          const disabledProviders = Array.isArray(config.disabled_providers)
            ? config.disabled_providers
                .filter((item): item is string => typeof item === "string")
                .filter((item) => item !== targetId)
            : []
          if (disabledProviders.length > 0) {
            config.disabled_providers = disabledProviders
          } else {
            delete config.disabled_providers
          }
        }
      )
      if (nextConfig.recoveredFromInvalid) {
        toast.warning(t("warnings.nativeJsonRecoveredOpenCode"))
      }

      const nextAuth = patchOpenCodeAuthJsonText(
        selectedDraft.openCodeAuthJsonText,
        (authObject) => {
          delete authObject[targetId]
        }
      )
      if (nextAuth.recoveredFromInvalid) {
        toast.warning(t("warnings.openCodeAuthRecovered"))
      }

      const nextOpenCode = extractOpenCodeConfigValues(
        nextConfig.configText,
        nextAuth.authJsonText
      )
      const nextDraft = {
        ...selectedDraft,
        configText: nextConfig.configText,
        openCodeAuthJsonText: nextAuth.authJsonText,
        model: nextOpenCode.model,
      }
      setConfigErrors((prev) => ({
        ...prev,
        [selectedAgent.agent_type]: null,
      }))
      setDrafts((prev) => ({
        ...prev,
        [selectedAgent.agent_type]: nextDraft,
      }))
      setOpenCodeProviderId((current) => (current === targetId ? "" : current))
      setOpenCodeNewModelIds((prev) => {
        if (typeof prev[targetId] === "undefined") return prev
        const next = { ...prev }
        delete next[targetId]
        return next
      })
      setOpenCodeModelConfigExpanded((prev) => {
        if (typeof prev[targetId] === "undefined") return prev
        const next = { ...prev }
        delete next[targetId]
        return next
      })
      setOpenCodeModelIdDrafts((prev) => {
        const prefix = `${targetId}:`
        const keys = Object.keys(prev).filter((key) => key.startsWith(prefix))
        if (keys.length === 0) return prev
        const next = { ...prev }
        for (const key of keys) {
          delete next[key]
        }
        return next
      })
      return {
        enabled: nextDraft.enabled,
        envText: nextDraft.envText,
        configText: nextDraft.configText,
        openCodeAuthJsonText: nextDraft.openCodeAuthJsonText,
      }
    },
    [selectedAgent, selectedDraft, t]
  )

  const confirmOpenCodeProviderDelete = useCallback(() => {
    const providerId = openCodeDeleteProviderId?.trim()
    if (!providerId) return
    const removed = handleOpenCodeRemoveProvider(providerId)
    setOpenCodeDeleteProviderId(null)
    if (
      !removed ||
      !selectedAgent ||
      selectedAgent.agent_type !== "open_code"
    ) {
      return
    }
    persistConfig(selectedAgent.agent_type, removed.configText, {
      openCodeAuthJsonText: removed.openCodeAuthJsonText,
    })
      .then(() => {
        toast.success(t("toasts.providerDeleted", { providerId }), {
          description: t("toasts.openCodeConfigSynced"),
        })
      })
      .catch((err) => {
        console.error("[Settings] remove opencode provider failed:", err)
        const message = err instanceof Error ? err.message : String(err)
        toast.error(t("toasts.providerDeleteFailed", { providerId }), {
          description: message,
        })
      })
  }, [
    handleOpenCodeRemoveProvider,
    openCodeDeleteProviderId,
    persistConfig,
    selectedAgent,
    t,
  ])

  const handleOpenCodeProviderStatusChange = useCallback(
    (providerId: string, enabled: boolean) => {
      const targetId = providerId.trim()
      if (!targetId) return
      handleOpenCodeConfigPatch((config) => {
        const enabledProviders = Array.isArray(config.enabled_providers)
          ? config.enabled_providers
              .filter((item): item is string => typeof item === "string")
              .map((item) => item.trim())
              .filter(Boolean)
          : []
        const disabledProviders = Array.isArray(config.disabled_providers)
          ? config.disabled_providers
              .filter((item): item is string => typeof item === "string")
              .map((item) => item.trim())
              .filter(Boolean)
          : []

        const nextEnabled = new Set(enabledProviders)
        const nextDisabled = new Set(disabledProviders)

        if (enabled) {
          nextEnabled.add(targetId)
          nextDisabled.delete(targetId)
        } else {
          nextDisabled.add(targetId)
          nextEnabled.delete(targetId)
        }

        const enabledArray = Array.from(nextEnabled)
        const disabledArray = Array.from(nextDisabled)
        if (enabledArray.length > 0) {
          config.enabled_providers = enabledArray
        } else {
          delete config.enabled_providers
        }
        if (disabledArray.length > 0) {
          config.disabled_providers = disabledArray
        } else {
          delete config.disabled_providers
        }
      })
    },
    [handleOpenCodeConfigPatch]
  )

  const handleOpenCodeProviderFieldChange = useCallback(
    (
      providerId: string,
      key: "name" | "api" | "npm" | "baseURL" | "apiKey",
      value: string
    ) => {
      const targetId = providerId.trim()
      if (!targetId) return
      handleOpenCodeConfigPatch((config) => {
        const providerRoot = asObjectRecord(config.provider) ?? {}
        if (!asObjectRecord(config.provider)) {
          config.provider = providerRoot
        }

        const currentProvider = asObjectRecord(providerRoot[targetId]) ?? {}
        if (!asObjectRecord(providerRoot[targetId])) {
          providerRoot[targetId] = currentProvider
        }
        const trimmed = value.trim()
        if (key === "baseURL" || key === "apiKey") {
          const options = asObjectRecord(currentProvider.options) ?? {}
          if (!asObjectRecord(currentProvider.options)) {
            currentProvider.options = options
          }
          if (trimmed) {
            options[key] = trimmed
          } else {
            delete options[key]
          }
          if (Object.keys(options).length === 0) {
            delete currentProvider.options
          }
          return
        }
        if (trimmed) {
          currentProvider[key] = trimmed
        } else {
          delete currentProvider[key]
        }
      })
      if (key === "apiKey" && selectedDraft) {
        const nextAuth = patchOpenCodeAuthJsonText(
          selectedDraft.openCodeAuthJsonText,
          (authObject) => {
            const entry = asObjectRecord(authObject[targetId]) ?? {}
            if (!asObjectRecord(authObject[targetId])) {
              authObject[targetId] = entry
            }
            const trimmed = value.trim()
            if (!trimmed) {
              delete entry.key
              if (entry.type === "api") delete entry.type
              if (Object.keys(entry).length === 0) {
                delete authObject[targetId]
              }
              return
            }
            entry.type = "api"
            entry.key = trimmed
          }
        )
        updateSelectedDraft((current) => ({
          ...current,
          openCodeAuthJsonText: nextAuth.authJsonText,
        }))
      }
    },
    [handleOpenCodeConfigPatch, selectedDraft, updateSelectedDraft]
  )

  const handleOpenCodeModelDraftChange = useCallback(
    (providerId: string, value: string) => {
      const targetId = providerId.trim()
      if (!targetId) return
      setOpenCodeNewModelIds((prev) => ({
        ...prev,
        [targetId]: value,
      }))
    },
    []
  )

  const handleOpenCodeAddModel = useCallback(
    (providerId: string) => {
      const targetProviderId = providerId.trim()
      if (!targetProviderId || !selectedOpenCodeConfig) return
      const nextModelId = (openCodeNewModelIds[targetProviderId] ?? "").trim()
      if (!nextModelId) return
      const targetProvider = selectedOpenCodeConfig.providers[targetProviderId]
      if (!targetProvider) return
      if (targetProvider.modelIds.includes(nextModelId)) {
        toast.error(t("errors.modelExists", { modelId: nextModelId }))
        return
      }
      handleOpenCodeConfigPatch((config) => {
        const providerRoot = asObjectRecord(config.provider) ?? {}
        if (!asObjectRecord(config.provider)) {
          config.provider = providerRoot
        }

        const currentProvider =
          asObjectRecord(providerRoot[targetProviderId]) ?? {}
        if (!asObjectRecord(providerRoot[targetProviderId])) {
          providerRoot[targetProviderId] = currentProvider
        }

        const modelsRoot = asObjectRecord(currentProvider.models) ?? {}
        if (!asObjectRecord(currentProvider.models)) {
          currentProvider.models = modelsRoot
        }
        modelsRoot[nextModelId] = {
          name: nextModelId,
        }
      })
      setOpenCodeNewModelIds((prev) => ({
        ...prev,
        [targetProviderId]: "",
      }))
    },
    [handleOpenCodeConfigPatch, openCodeNewModelIds, selectedOpenCodeConfig, t]
  )

  const handleOpenCodeRemoveModel = useCallback(
    (providerId: string, modelId: string) => {
      const targetProviderId = providerId.trim()
      const targetModelId = modelId.trim()
      if (!targetProviderId || !targetModelId) return
      handleOpenCodeConfigPatch((config) => {
        const providerRoot = asObjectRecord(config.provider)
        if (!providerRoot) return
        const currentProvider = asObjectRecord(providerRoot[targetProviderId])
        if (!currentProvider) return
        const modelsRoot = asObjectRecord(currentProvider.models)
        if (!modelsRoot) return
        delete modelsRoot[targetModelId]
        if (Object.keys(modelsRoot).length === 0) {
          delete currentProvider.models
        }
      })
      const draftKey = `${targetProviderId}:${targetModelId}`
      setOpenCodeModelIdDrafts((prev) => {
        if (typeof prev[draftKey] === "undefined") return prev
        const next = { ...prev }
        delete next[draftKey]
        return next
      })
    },
    [handleOpenCodeConfigPatch]
  )

  const handleOpenCodeModelIdDraftChange = useCallback(
    (providerId: string, modelId: string, value: string) => {
      const targetProviderId = providerId.trim()
      const targetModelId = modelId.trim()
      if (!targetProviderId || !targetModelId) return
      const draftKey = `${targetProviderId}:${targetModelId}`
      setOpenCodeModelIdDrafts((prev) => ({
        ...prev,
        [draftKey]: value,
      }))
    },
    []
  )

  const handleOpenCodeModelIdCommit = useCallback(
    (providerId: string, modelId: string) => {
      const targetProviderId = providerId.trim()
      const targetModelId = modelId.trim()
      if (!targetProviderId || !targetModelId || !selectedOpenCodeConfig) return
      const draftKey = `${targetProviderId}:${targetModelId}`
      const rawDraft = openCodeModelIdDrafts[draftKey]
      if (typeof rawDraft !== "string") return
      const nextModelId = rawDraft.trim()

      if (!nextModelId || nextModelId === targetModelId) {
        setOpenCodeModelIdDrafts((prev) => {
          const next = { ...prev }
          delete next[draftKey]
          return next
        })
        return
      }

      if (!/^[A-Za-z0-9_.:-]+$/.test(nextModelId)) {
        toast.error(t("errors.modelIdPattern"))
        return
      }

      const targetProvider = selectedOpenCodeConfig.providers[targetProviderId]
      if (!targetProvider) return
      if (targetProvider.modelIds.includes(nextModelId)) {
        toast.error(t("errors.modelExists", { modelId: nextModelId }))
        return
      }

      handleOpenCodeConfigPatch((config) => {
        const providerRoot = asObjectRecord(config.provider) ?? {}
        if (!asObjectRecord(config.provider)) {
          config.provider = providerRoot
        }
        const currentProvider =
          asObjectRecord(providerRoot[targetProviderId]) ?? {}
        if (!asObjectRecord(providerRoot[targetProviderId])) {
          providerRoot[targetProviderId] = currentProvider
        }
        const modelsRoot = asObjectRecord(currentProvider.models) ?? {}
        if (!asObjectRecord(currentProvider.models)) {
          currentProvider.models = modelsRoot
        }
        const currentModel = asObjectRecord(modelsRoot[targetModelId]) ?? {}
        if (!asObjectRecord(modelsRoot[targetModelId])) return
        delete currentModel.id
        modelsRoot[nextModelId] = currentModel
        delete modelsRoot[targetModelId]
      })

      setOpenCodeModelIdDrafts((prev) => {
        const next = { ...prev }
        delete next[draftKey]
        return next
      })
    },
    [
      handleOpenCodeConfigPatch,
      openCodeModelIdDrafts,
      selectedOpenCodeConfig,
      t,
    ]
  )

  const handleOpenCodeModelFieldChange = useCallback(
    (providerId: string, modelId: string, value: string) => {
      const targetProviderId = providerId.trim()
      const targetModelId = modelId.trim()
      if (!targetProviderId || !targetModelId) return
      handleOpenCodeConfigPatch((config) => {
        const providerRoot = asObjectRecord(config.provider) ?? {}
        if (!asObjectRecord(config.provider)) {
          config.provider = providerRoot
        }
        const currentProvider =
          asObjectRecord(providerRoot[targetProviderId]) ?? {}
        if (!asObjectRecord(providerRoot[targetProviderId])) {
          providerRoot[targetProviderId] = currentProvider
        }
        const modelsRoot = asObjectRecord(currentProvider.models) ?? {}
        if (!asObjectRecord(currentProvider.models)) {
          currentProvider.models = modelsRoot
        }
        const currentModel = asObjectRecord(modelsRoot[targetModelId]) ?? {}
        if (!asObjectRecord(modelsRoot[targetModelId])) {
          modelsRoot[targetModelId] = currentModel
        }
        const trimmed = value.trim()
        if (trimmed) {
          currentModel.name = trimmed
        } else {
          delete currentModel.name
        }
        // Cleanup legacy schema written by earlier versions.
        delete currentModel.id
      })
    },
    [handleOpenCodeConfigPatch]
  )

  const handleCodexAuthJsonTextChange = useCallback(
    (nextText: string) => {
      if (!selectedAgent || selectedAgent.agent_type !== "codex") return
      const important = extractCodexImportantValues(
        nextText,
        selectedDraft?.codexConfigTomlText ?? ""
      )
      updateSelectedDraft((current) => ({
        ...current,
        codexAuthMode: inferCodexAuthMode(nextText),
        codexAuthJsonText: nextText,
        apiBaseUrl: important.apiBaseUrl,
        apiKey: important.apiKey ?? current.apiKey,
        model: important.model,
        codexModelProvider: important.modelProvider,
        codexProviderOptions: important.providerOptions,
        codexReasoningEffort: important.reasoningEffort,
        codexSupportsWebsockets: important.supportsWebsockets,
      }))
    },
    [selectedAgent, selectedDraft, updateSelectedDraft]
  )

  const handleCodexConfigTomlTextChange = useCallback(
    (nextText: string) => {
      if (!selectedAgent || selectedAgent.agent_type !== "codex") return
      const important = extractCodexImportantValues(
        selectedDraft?.codexAuthJsonText ?? "",
        nextText
      )
      updateSelectedDraft((current) => ({
        ...current,
        codexConfigTomlText: nextText,
        apiBaseUrl: important.apiBaseUrl,
        apiKey: important.apiKey ?? current.apiKey,
        model: important.model,
        codexModelProvider: important.modelProvider,
        codexProviderOptions: important.providerOptions,
        codexReasoningEffort: important.reasoningEffort,
        codexSupportsWebsockets: important.supportsWebsockets,
      }))
    },
    [selectedAgent, selectedDraft, updateSelectedDraft]
  )

  const handleCodexAuthModeChange = useCallback(
    (nextMode: CodexAuthMode) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "codex"
      )
        return

      if (nextMode === "chatgpt_subscription") {
        // Official subscription: set auth_mode to chatgpt, OPENAI_API_KEY to null
        const nextAuth = patchCodexAuthJsonText(
          selectedDraft.codexAuthJsonText,
          { authMode: "chatgpt" }
        )
        const nextAuthJsonText = nextAuth.authJsonText
        let nextConfigTomlText = updateTomlRootStringKey(
          selectedDraft.codexConfigTomlText,
          "model_provider",
          ""
        )
        nextConfigTomlText = removeTomlSection(
          nextConfigTomlText,
          `model_providers.${CODEX_DEFAULT_MODEL_PROVIDER}`
        )
        const synced = extractCodexImportantValues(
          nextAuthJsonText,
          nextConfigTomlText
        )
        updateSelectedDraft((current) => ({
          ...current,
          codexAuthMode: nextMode,
          modelProviderId: null,
          codexAuthJsonText: nextAuthJsonText,
          codexConfigTomlText: nextConfigTomlText,
          envText: patchEnvText(current.envText, {
            OPENAI_API_KEY: "",
            OPENAI_BASE_URL: "",
          }),
          apiBaseUrl: "",
          apiKey: "",
          model: synced.model,
          codexModelProvider: synced.modelProvider,
          codexProviderOptions: synced.providerOptions,
          codexReasoningEffort: synced.reasoningEffort,
          codexSupportsWebsockets: synced.supportsWebsockets,
        }))
        return
      }

      // "api_key" or "model_provider": ensure model_provider = "codeg" in toml
      const nextConfigTomlText = patchCodexConfigTomlText(
        selectedDraft.codexConfigTomlText,
        { modelProvider: CODEX_DEFAULT_MODEL_PROVIDER }
      )
      const nextAuthPatch = patchCodexAuthJsonText(
        selectedDraft.codexAuthJsonText,
        { authMode: null }
      )
      const nextAuthJsonText = nextAuthPatch.authJsonText
      const synced = extractCodexImportantValues(
        nextAuthJsonText,
        nextConfigTomlText
      )
      updateSelectedDraft((current) => ({
        ...current,
        codexAuthMode: nextMode,
        modelProviderId:
          nextMode === "model_provider" ? current.modelProviderId : null,
        codexAuthJsonText: nextAuthJsonText,
        codexConfigTomlText: nextConfigTomlText,
        apiBaseUrl: synced.apiBaseUrl,
        apiKey: synced.apiKey ?? current.apiKey,
        model: synced.model,
        codexModelProvider: CODEX_DEFAULT_MODEL_PROVIDER,
        codexProviderOptions: synced.providerOptions,
        codexReasoningEffort: synced.reasoningEffort,
        codexSupportsWebsockets: synced.supportsWebsockets,
      }))
    },
    [selectedAgent, selectedDraft, updateSelectedDraft]
  )

  const handleCodexImportantConfigChange = useCallback(
    (
      key: "apiBaseUrl" | "apiKey" | "model" | "reasoningEffort",
      value: string
    ) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "codex"
      )
        return
      const nextAuth =
        key === "apiKey"
          ? patchCodexAuthJsonText(selectedDraft.codexAuthJsonText, {
              apiKey: value,
            })
          : {
              authJsonText: selectedDraft.codexAuthJsonText,
              recoveredFromInvalid: false,
            }
      const nextToml =
        key === "apiBaseUrl"
          ? patchCodexConfigTomlText(selectedDraft.codexConfigTomlText, {
              apiBaseUrl: value,
              modelProvider: selectedDraft.codexModelProvider,
              modelReasoningEffort: selectedDraft.codexReasoningEffort,
            })
          : key === "model"
            ? patchCodexConfigTomlText(selectedDraft.codexConfigTomlText, {
                model: value,
                modelReasoningEffort: selectedDraft.codexReasoningEffort,
              })
            : key === "reasoningEffort"
              ? patchCodexConfigTomlText(selectedDraft.codexConfigTomlText, {
                  modelReasoningEffort: value,
                })
              : selectedDraft.codexConfigTomlText
      if (nextAuth.recoveredFromInvalid) {
        toast.warning(t("warnings.authRecoveredStructured"))
      }
      const synced = extractCodexImportantValues(
        nextAuth.authJsonText,
        nextToml
      )
      updateSelectedDraft((current) => ({
        ...(key === "reasoningEffort"
          ? {
              ...current,
              codexReasoningEffort:
                normalizeCodexReasoningEffort(value) ??
                CODEX_DEFAULT_REASONING_EFFORT,
            }
          : applyImportantFieldToDraft(current, key, value)),
        apiBaseUrl: synced.apiBaseUrl,
        apiKey: synced.apiKey ?? current.apiKey,
        model: synced.model,
        codexModelProvider: synced.modelProvider,
        codexProviderOptions: synced.providerOptions,
        codexReasoningEffort: synced.reasoningEffort,
        codexSupportsWebsockets: synced.supportsWebsockets,
        codexAuthJsonText: nextAuth.authJsonText,
        codexConfigTomlText: nextToml,
      }))
    },
    [selectedAgent, selectedDraft, t, updateSelectedDraft]
  )

  const handleCodexSupportsWebsocketsChange = useCallback(
    (enabled: boolean) => {
      if (
        !selectedAgent ||
        !selectedDraft ||
        selectedAgent.agent_type !== "codex"
      )
        return
      const nextToml = patchCodexConfigTomlText(
        selectedDraft.codexConfigTomlText,
        {
          modelProvider: selectedDraft.codexModelProvider,
          supportsWebsockets: enabled,
        }
      )
      const synced = extractCodexImportantValues(
        selectedDraft.codexAuthJsonText,
        nextToml
      )
      updateSelectedDraft((current) => ({
        ...current,
        apiBaseUrl: synced.apiBaseUrl,
        apiKey: synced.apiKey ?? current.apiKey,
        model: synced.model,
        codexModelProvider: synced.modelProvider,
        codexProviderOptions: synced.providerOptions,
        codexReasoningEffort: synced.reasoningEffort,
        codexSupportsWebsockets: synced.supportsWebsockets,
        codexConfigTomlText: nextToml,
      }))
    },
    [selectedAgent, selectedDraft, updateSelectedDraft]
  )

  if (loadingAgents) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 mr-2 animate-spin" />
        {t("loadingAgents")}
      </div>
    )
  }

  return (
    <div className="h-full flex flex-col">
      <div className="flex items-center justify-between gap-3 pb-4">
        <div>
          <h2 className="text-base font-semibold">{t("title")}</h2>
          <p className="text-xs text-muted-foreground mt-1">
            {t("description")}
          </p>
        </div>
      </div>

      {loadingError && (
        <div className="mb-3 rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
          {loadingError}
        </div>
      )}

      <div className="flex-1 min-h-0 grid gap-3 lg:grid-cols-[minmax(240px,320px)_1fr]">
        <div className="min-h-0 min-w-0 rounded-lg border bg-card flex flex-col overflow-hidden">
          <div className="border-b px-3 py-2 text-xs font-medium text-muted-foreground">
            {t("agentList")}
          </div>
          <Reorder.Group
            as="div"
            axis="y"
            values={sortedAgents}
            onReorder={handleReorder}
            ref={agentListRef}
            className="flex-1 min-h-0 overflow-y-auto space-y-2 p-2"
          >
            {sortedAgents.map((agent) => {
              const current = checkState[agent.agent_type]
              const isChecking = Boolean(checking[agent.agent_type])
              const draft = drafts[agent.agent_type] ?? buildAgentDraft(agent)
              const allChecks = getAgentChecks(agent, current)
              const summary = summarizeChecks(allChecks)
              const displaySummary: CheckStatus | "unchecked" | "checking" =
                isChecking ? "checking" : summary
              const statusLabel =
                displaySummary === "unchecked"
                  ? t("status.unchecked")
                  : displaySummary === "checking"
                    ? "Checking"
                    : displaySummary.toUpperCase()
              const statusToneClass = !draft.enabled
                ? "border-muted-foreground/30 bg-muted/30 text-muted-foreground"
                : displaySummary === "pass"
                  ? "border-green-500/40 bg-green-500/10 text-green-600 dark:text-green-400"
                  : displaySummary === "fail"
                    ? "border-red-500/40 bg-red-500/10 text-red-500"
                    : displaySummary === "warn"
                      ? "border-yellow-500/40 bg-yellow-500/10 text-yellow-600 dark:text-yellow-400"
                      : displaySummary === "checking"
                        ? "border-blue-500/40 bg-blue-500/10 text-blue-600 dark:text-blue-400"
                        : "border-muted-foreground/30 bg-muted/30 text-muted-foreground"

              return (
                <AgentReorderItem
                  key={agent.agent_type}
                  agent={agent}
                  selected={selectedAgentType === agent.agent_type}
                  reordering={reordering}
                  dragging={dragging}
                  onDragStart={(agentType) => {
                    setDragging(agentType)
                  }}
                  onDragEnd={() => {
                    const order = pendingOrderRef.current
                    pendingOrderRef.current = null
                    setDragging(null)
                    if (order && !reordering) {
                      persistReorder(order).catch((err) => {
                        console.error("[Settings] reorder agents failed:", err)
                      })
                    }
                  }}
                  onSelect={(agentType) => {
                    setSelectedAgentType(agentType)
                  }}
                >
                  {(startDrag) => (
                    <div className="flex items-center justify-between gap-2 overflow-hidden">
                      <div className="min-w-0 flex items-center gap-2">
                        <button
                          type="button"
                          className="text-muted-foreground cursor-grab active:cursor-grabbing rounded p-0.5 hover:bg-muted"
                          title={t("actions.dragSort")}
                          aria-label={t("actions.dragSortAgent", {
                            name: agent.name,
                          })}
                          onPointerDown={startDrag}
                          onClick={(event) => {
                            event.stopPropagation()
                          }}
                          disabled={reordering}
                        >
                          <GripVertical className="h-3.5 w-3.5" />
                        </button>
                        <AgentIcon
                          agentType={agent.agent_type}
                          className="h-4 w-4"
                        />
                        <span className="text-sm font-medium truncate">
                          {agent.name}
                        </span>
                        {draft.enabled && (
                          <span
                            className="h-2 w-2 rounded-full bg-emerald-500 shrink-0"
                            aria-label={t("status.agentEnabledAria", {
                              name: agent.name,
                            })}
                            title={t("status.enabled")}
                          />
                        )}
                      </div>

                      <div className="flex items-center gap-2 shrink-0">
                        <Badge
                          variant="outline"
                          className={cn(
                            "h-6 px-2 inline-flex items-center gap-1 text-xs leading-none",
                            statusToneClass
                          )}
                        >
                          <span>{statusLabel}</span>
                          {displaySummary === "checking" && (
                            <Loader2 className="h-3.5 w-3.5 animate-spin shrink-0" />
                          )}
                          {!isChecking && (
                            <button
                              type="button"
                              className="inline-flex h-4 w-4 items-center justify-center rounded hover:bg-black/10 dark:hover:bg-white/10"
                              title={t("actions.refreshCheck")}
                              aria-label={t("actions.refreshCheckAgent", {
                                name: agent.name,
                              })}
                              onClick={(event) => {
                                event.stopPropagation()
                                runPreflight(agent.agent_type, true).catch(
                                  (err) => {
                                    console.error(
                                      "[Settings] single preflight failed:",
                                      err
                                    )
                                  }
                                )
                              }}
                            >
                              <RefreshCw className="h-3 w-3 shrink-0" />
                            </button>
                          )}
                        </Badge>
                      </div>
                    </div>
                  )}
                </AgentReorderItem>
              )
            })}
          </Reorder.Group>
        </div>

        <div className="min-h-0 min-w-0 rounded-lg border bg-card">
          {selectedAgent && selectedDraft ? (
            <div className="h-full flex flex-col">
              <div className="border-b px-4 py-3">
                <div className="flex items-center justify-between gap-3">
                  <div className="min-w-0 flex items-center gap-2">
                    <AgentIcon
                      agentType={selectedAgent.agent_type}
                      className="h-5 w-5"
                    />
                    <h3 className="text-sm font-semibold truncate">
                      {selectedAgent.name}
                    </h3>
                    <Badge variant="outline" className="shrink-0">
                      {selectedAgent.distribution_type}
                    </Badge>
                  </div>
                  <div className="flex items-center shrink-0">
                    <button
                      type="button"
                      role="switch"
                      aria-checked={selectedDraft.enabled}
                      aria-label={t("status.agentEnabledSwitch", {
                        name: selectedAgent.name,
                      })}
                      title={
                        selectedDraft.enabled
                          ? t("actions.clickDisable", {
                              name: selectedAgent.name,
                            })
                          : t("actions.clickEnable", {
                              name: selectedAgent.name,
                            })
                      }
                      disabled={selectedIsSaving}
                      className={cn(
                        "relative inline-flex h-5 w-9 items-center rounded-full transition-colors",
                        selectedDraft.enabled
                          ? "bg-primary"
                          : "bg-muted-foreground/30",
                        selectedIsSaving && "cursor-not-allowed opacity-60"
                      )}
                      onClick={() => {
                        const nextEnabled = !selectedDraft.enabled
                        const nextDraft = {
                          ...selectedDraft,
                          enabled: nextEnabled,
                        }
                        setDrafts((prev) => ({
                          ...prev,
                          [selectedAgent.agent_type]: nextDraft,
                        }))
                        persistEnv(
                          selectedAgent.agent_type,
                          nextEnabled,
                          nextDraft.envText,
                          nextDraft.modelProviderId
                        ).catch((err) => {
                          console.error(
                            "[Settings] persist enabled failed:",
                            err
                          )
                          const message =
                            err instanceof Error ? err.message : String(err)
                          toast.error(t("toasts.saveAgentSwitchFailed"), {
                            description: message,
                          })
                        })
                      }}
                    >
                      <span
                        className={cn(
                          "inline-block h-4 w-4 rounded-full bg-background shadow-sm transition-transform",
                          selectedDraft.enabled
                            ? "translate-x-4"
                            : "translate-x-0.5"
                        )}
                      />
                    </button>
                  </div>
                </div>
                <p className="mt-2 text-xs text-muted-foreground">
                  {selectedAgent.description}
                </p>
              </div>

              <div className="flex-1 overflow-y-auto p-4 space-y-4">
                <div className="space-y-2">
                  {selectedCurrent?.error && (
                    <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400 flex items-start gap-2">
                      <AlertCircle className="h-3.5 w-3.5 mt-0.5 shrink-0" />
                      <span className="break-all">{selectedCurrent.error}</span>
                    </div>
                  )}
                  <div className="text-[11px] text-muted-foreground flex items-center gap-1">
                    <CheckCircle2 className="h-3 w-3" />
                    {t("preflight.count", { count: selectedChecks.length })}
                  </div>
                  {selectedChecks.length > 0 ? (
                    selectedChecks.map((check) =>
                      renderCheck(selectedAgent, check)
                    )
                  ) : (
                    <div className="text-xs text-muted-foreground">
                      {t("preflight.notRun")}
                    </div>
                  )}
                </div>

                <div className="space-y-2">
                  <label className="text-xs font-medium">{t("envVars")}</label>
                  <div className="relative group">
                    <Textarea
                      value={selectedDraft.envText}
                      onChange={(event) => {
                        updateSelectedDraft((current) => ({
                          ...current,
                          envText: event.target.value,
                        }))
                      }}
                      placeholder={"KEY1=VALUE1\nKEY2=VALUE2"}
                      className="min-h-24"
                    />
                    <div className="pointer-events-none absolute inset-0 rounded-md bg-background/10 backdrop-blur-[3px] transition-opacity duration-200 group-focus-within:opacity-0" />
                  </div>
                  <div className="flex justify-end">
                    <Button
                      size="sm"
                      onClick={() => {
                        persistEnv(
                          selectedAgent.agent_type,
                          selectedDraft.enabled,
                          selectedDraft.envText,
                          selectedDraft.modelProviderId
                        )
                          .then(() => {
                            toast.success(t("toasts.configSaved"), {
                              description: t("toasts.configSavedHint"),
                            })
                          })
                          .catch((err) => {
                            console.error("[Settings] save env failed:", err)
                            const message =
                              err instanceof Error ? err.message : String(err)
                            toast.error(t("toasts.saveEnvFailed"), {
                              description: message,
                            })
                          })
                      }}
                      disabled={selectedIsSavingEnv}
                    >
                      {selectedIsSavingEnv ? (
                        <>
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                          {t("actions.saving")}
                        </>
                      ) : (
                        <>
                          <Save className="h-3.5 w-3.5" />
                          {t("actions.saveEnvVars")}
                        </>
                      )}
                    </Button>
                  </div>
                </div>

                {selectedAgent.agent_type === "codex" ? (
                  <div className="space-y-3 rounded-md border bg-muted/10 p-3">
                    <div>
                      <label className="text-xs font-medium">
                        {t("configManagement")}
                      </label>
                      <p className="mt-1 text-[11px] text-muted-foreground">
                        {t("codex.configDescription")}
                      </p>
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        {t("codex.authMode")}
                      </label>
                      <Select
                        value={selectedDraft.codexAuthMode}
                        onValueChange={(value) => {
                          if (
                            CODEX_AUTH_MODES.includes(value as CodexAuthMode)
                          ) {
                            handleCodexAuthModeChange(value as CodexAuthMode)
                          }
                        }}
                      >
                        <SelectTrigger className="w-full">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent align="start">
                          {CODEX_AUTH_MODES.map((mode) => (
                            <SelectItem key={mode} value={mode}>
                              {mode === "chatgpt_subscription"
                                ? t("authModeOfficialSubscription")
                                : mode === "model_provider"
                                  ? t("authModeModelProvider")
                                  : t("authModeCustomEndpoint")}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <p className="text-[11px] text-muted-foreground">
                        {selectedDraft.codexAuthMode === "chatgpt_subscription"
                          ? t("codex.chatgptSubscriptionHint")
                          : selectedDraft.codexAuthMode === "model_provider"
                            ? t("modelProviderHint")
                            : t("authModeCustomEndpointHint")}
                      </p>
                    </div>

                    {selectedDraft.codexAuthMode === "model_provider" && (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          {t("selectModelProvider")}
                        </label>
                        {selectedModelProviders.length > 0 ? (
                          <Select
                            value={
                              selectedDraft.modelProviderId != null
                                ? String(selectedDraft.modelProviderId)
                                : ""
                            }
                            onValueChange={handleModelProviderSelect}
                          >
                            <SelectTrigger className="w-full">
                              <SelectValue
                                placeholder={t("selectModelProvider")}
                              />
                            </SelectTrigger>
                            <SelectContent align="start">
                              {selectedModelProviders.map((provider) => (
                                <SelectItem
                                  key={provider.id}
                                  value={String(provider.id)}
                                >
                                  {provider.name}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        ) : (
                          <p className="text-[11px] text-muted-foreground">
                            {t("noModelProviderAvailable")}
                          </p>
                        )}
                      </div>
                    )}

                    {selectedDraft.codexAuthMode === "api_key" && (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          API URL
                        </label>
                        <Input
                          value={selectedDraft.apiBaseUrl}
                          onChange={(event) => {
                            handleCodexImportantConfigChange(
                              "apiBaseUrl",
                              event.target.value
                            )
                          }}
                          placeholder="https://api.openai.com/v1"
                        />
                      </div>
                    )}

                    {selectedDraft.codexAuthMode === "api_key" && (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          API Key
                        </label>
                        <div className="flex items-center gap-2">
                          <Input
                            type={
                              showApiKeys[selectedAgent.agent_type]
                                ? "text"
                                : "password"
                            }
                            value={selectedDraft.apiKey}
                            onChange={(event) => {
                              handleCodexImportantConfigChange(
                                "apiKey",
                                event.target.value
                              )
                            }}
                            placeholder="sk-..."
                          />
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() => {
                              setShowApiKeys((prev) => ({
                                ...prev,
                                [selectedAgent.agent_type]:
                                  !prev[selectedAgent.agent_type],
                              }))
                            }}
                            title={
                              showApiKeys[selectedAgent.agent_type]
                                ? t("actions.hideApiKey")
                                : t("actions.showApiKey")
                            }
                          >
                            {showApiKeys[selectedAgent.agent_type] ? (
                              <EyeOff className="h-3.5 w-3.5" />
                            ) : (
                              <Eye className="h-3.5 w-3.5" />
                            )}
                          </Button>
                        </div>
                      </div>
                    )}

                    {(selectedDraft.codexAuthMode === "api_key" ||
                      selectedDraft.codexAuthMode === "model_provider") && (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          {t("codex.modelName")}
                        </label>
                        <Input
                          value={selectedDraft.model}
                          onChange={(event) => {
                            handleCodexImportantConfigChange(
                              "model",
                              event.target.value
                            )
                          }}
                          placeholder="gpt-5 / gpt-5-mini"
                        />
                      </div>
                    )}

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        Reasoning Effort
                      </label>
                      <Select
                        value={selectedDraft.codexReasoningEffort}
                        onValueChange={(nextValue) => {
                          handleCodexImportantConfigChange(
                            "reasoningEffort",
                            nextValue
                          )
                        }}
                      >
                        <SelectTrigger className="w-full">
                          <SelectValue
                            placeholder={t("codex.selectReasoningEffort")}
                          />
                        </SelectTrigger>
                        <SelectContent align="start">
                          {CODEX_REASONING_EFFORT_OPTIONS.map((option) => (
                            <SelectItem key={option.value} value={option.value}>
                              {option.label}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <p className="text-[11px] text-muted-foreground">
                        {selectedCodexReasoningEffortOption?.description ??
                          "Greater reasoning depth for complex problems"}
                      </p>
                    </div>

                    <div className="space-y-1.5">
                      <div className="flex items-center justify-between rounded-md border px-3 py-2">
                        <label className="text-[11px] text-muted-foreground">
                          {t("codex.enableWebsocket")}
                        </label>
                        <Switch
                          checked={selectedDraft.codexSupportsWebsockets}
                          onCheckedChange={handleCodexSupportsWebsocketsChange}
                          aria-label={t("codex.enableWebsocketAria")}
                        />
                      </div>
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        {t("codex.authJsonNative")}
                      </label>
                      <Textarea
                        value={selectedDraft.codexAuthJsonText}
                        onChange={(event) => {
                          handleCodexAuthJsonTextChange(event.target.value)
                        }}
                        placeholder={`{
  "OPENAI_API_KEY": "sk-..."
}`}
                        className="min-h-28 font-mono text-xs"
                      />
                      {selectedCodexAuthError && (
                        <div className="rounded-md border border-red-500/30 bg-red-500/5 px-2.5 py-1.5 text-[11px] text-red-400">
                          {selectedCodexAuthError}
                        </div>
                      )}
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        {t("codex.configTomlNative")}
                      </label>
                      <Textarea
                        value={selectedDraft.codexConfigTomlText}
                        onChange={(event) => {
                          handleCodexConfigTomlTextChange(event.target.value)
                        }}
                        placeholder={`disable_response_storage = true
model = "gpt-5"
model_reasoning_effort = "high"
model_provider = "codeg"

[features]
responses_websockets_v2 = true

[model_providers.codeg]
base_url = "https://api.openai.com/v1"
supports_websockets = true`}
                        className="min-h-40 font-mono text-xs"
                      />
                    </div>

                    <div className="flex justify-end">
                      <Button
                        size="sm"
                        onClick={() => {
                          if (selectedMissingModelProvider) {
                            toast.error(t("toasts.modelProviderRequired"))
                            return
                          }
                          const codexEnvText =
                            selectedDraft.codexAuthMode ===
                            "chatgpt_subscription"
                              ? patchEnvText(selectedDraft.envText, {
                                  OPENAI_API_KEY: "",
                                  OPENAI_BASE_URL: "",
                                })
                              : selectedDraft.envText
                          Promise.all([
                            persistEnv(
                              selectedAgent.agent_type,
                              selectedDraft.enabled,
                              codexEnvText,
                              selectedDraft.modelProviderId
                            ),
                            persistConfig(
                              selectedAgent.agent_type,
                              selectedDraft.configText,
                              {
                                codexAuthJsonText:
                                  selectedDraft.codexAuthJsonText,
                                codexConfigTomlText:
                                  selectedDraft.codexConfigTomlText,
                              }
                            ),
                          ])
                            .then(() => {
                              toast.success(t("toasts.codexSaved"), {
                                description: t("toasts.configSavedHint"),
                              })
                            })
                            .catch((err) => {
                              console.error(
                                "[Settings] save codex native config failed:",
                                err
                              )
                              const message =
                                err instanceof Error ? err.message : String(err)
                              toast.error(t("toasts.saveCodexNativeFailed"), {
                                description: message,
                              })
                            })
                        }}
                        disabled={selectedIsSavingEnv || selectedIsSavingConfig}
                      >
                        {selectedIsSavingEnv || selectedIsSavingConfig ? (
                          <>
                            <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            {t("actions.saving")}
                          </>
                        ) : (
                          <>
                            <Save className="h-3.5 w-3.5" />
                            {t("actions.saveCodexConfig")}
                          </>
                        )}
                      </Button>
                    </div>
                  </div>
                ) : selectedAgent.agent_type === "gemini" ? (
                  <div className="space-y-3 rounded-md border bg-muted/10 p-3">
                    <div>
                      <label className="text-xs font-medium">
                        {t("gemini.authConfig")}
                      </label>
                      <p className="mt-1 text-[11px] text-muted-foreground">
                        {t("gemini.authConfigDescription")}
                      </p>
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        {t("gemini.authMode")}
                      </label>
                      <Select
                        value={selectedDraft.geminiAuthMode}
                        onValueChange={(value) => {
                          if (
                            GEMINI_AUTH_MODES.includes(value as GeminiAuthMode)
                          ) {
                            handleGeminiAuthModeChange(value as GeminiAuthMode)
                          }
                        }}
                      >
                        <SelectTrigger className="w-full">
                          <SelectValue
                            placeholder={t("gemini.selectAuthMode")}
                          />
                        </SelectTrigger>
                        <SelectContent align="start">
                          {GEMINI_AUTH_MODES.map((mode) => (
                            <SelectItem key={mode} value={mode}>
                              {geminiAuthModeLabel(mode)}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                      <p className="text-[11px] text-muted-foreground">
                        {geminiAuthModeHint(selectedDraft.geminiAuthMode)}
                      </p>
                    </div>

                    {selectedDraft.geminiAuthMode === "model_provider" && (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          {t("selectModelProvider")}
                        </label>
                        {selectedModelProviders.length > 0 ? (
                          <Select
                            value={
                              selectedDraft.modelProviderId != null
                                ? String(selectedDraft.modelProviderId)
                                : ""
                            }
                            onValueChange={handleModelProviderSelect}
                          >
                            <SelectTrigger className="w-full">
                              <SelectValue
                                placeholder={t("selectModelProvider")}
                              />
                            </SelectTrigger>
                            <SelectContent align="start">
                              {selectedModelProviders.map((provider) => (
                                <SelectItem
                                  key={provider.id}
                                  value={String(provider.id)}
                                >
                                  {provider.name}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        ) : (
                          <p className="text-[11px] text-muted-foreground">
                            {t("noModelProviderAvailable")}
                          </p>
                        )}
                      </div>
                    )}

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        Model
                      </label>
                      <Input
                        value={selectedDraft.model}
                        onChange={(event) => {
                          handleGeminiFieldChange("model", event.target.value)
                        }}
                        placeholder="gemini-3-pro-preview"
                      />
                      <p className="text-[11px] text-muted-foreground">
                        {t("modelHintDefault")}
                      </p>
                    </div>

                    {(selectedDraft.geminiAuthMode === "custom" ||
                      selectedDraft.geminiAuthMode === "model_provider") && (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          GOOGLE_GEMINI_BASE_URL
                        </label>
                        <Input
                          value={selectedDraft.apiBaseUrl}
                          readOnly={
                            selectedDraft.geminiAuthMode === "model_provider"
                          }
                          onChange={(event) => {
                            handleGeminiFieldChange(
                              "apiBaseUrl",
                              event.target.value
                            )
                          }}
                          placeholder="https://your-gemini-endpoint.example.com"
                        />
                      </div>
                    )}

                    {(selectedDraft.geminiAuthMode === "custom" ||
                      selectedDraft.geminiAuthMode === "gemini_api_key" ||
                      selectedDraft.geminiAuthMode === "model_provider" ||
                      selectedDraft.geminiAuthMode === "vertex_api_key") && (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          {selectedDraft.geminiAuthMode === "vertex_api_key"
                            ? "GOOGLE_API_KEY"
                            : "GEMINI_API_KEY"}
                        </label>
                        <div className="flex items-center gap-2">
                          <Input
                            type={
                              showApiKeys[selectedAgent.agent_type]
                                ? "text"
                                : "password"
                            }
                            value={
                              selectedDraft.geminiAuthMode === "vertex_api_key"
                                ? selectedDraft.googleApiKey
                                : selectedDraft.geminiApiKey
                            }
                            readOnly={
                              selectedDraft.geminiAuthMode === "model_provider"
                            }
                            onChange={(event) => {
                              if (
                                selectedDraft.geminiAuthMode ===
                                "vertex_api_key"
                              ) {
                                handleGeminiFieldChange(
                                  "googleApiKey",
                                  event.target.value
                                )
                                return
                              }
                              handleGeminiFieldChange(
                                "geminiApiKey",
                                event.target.value
                              )
                            }}
                            placeholder="AIza..."
                          />
                          <Button
                            type="button"
                            variant="outline"
                            size="sm"
                            onClick={() => {
                              setShowApiKeys((prev) => ({
                                ...prev,
                                [selectedAgent.agent_type]:
                                  !prev[selectedAgent.agent_type],
                              }))
                            }}
                            title={
                              showApiKeys[selectedAgent.agent_type]
                                ? t("actions.hideKey")
                                : t("actions.showKey")
                            }
                          >
                            {showApiKeys[selectedAgent.agent_type] ? (
                              <EyeOff className="h-3.5 w-3.5" />
                            ) : (
                              <Eye className="h-3.5 w-3.5" />
                            )}
                          </Button>
                        </div>
                      </div>
                    )}

                    {(selectedDraft.geminiAuthMode === "vertex_adc" ||
                      selectedDraft.geminiAuthMode ===
                        "vertex_service_account" ||
                      selectedDraft.geminiAuthMode === "vertex_api_key") && (
                      <div className="grid gap-3 md:grid-cols-2">
                        <div className="space-y-1.5">
                          <label className="text-[11px] text-muted-foreground">
                            GOOGLE_CLOUD_PROJECT
                          </label>
                          <Input
                            value={selectedDraft.googleCloudProject}
                            onChange={(event) => {
                              handleGeminiFieldChange(
                                "googleCloudProject",
                                event.target.value
                              )
                            }}
                            placeholder="my-gcp-project-id"
                          />
                        </div>
                        <div className="space-y-1.5">
                          <label className="text-[11px] text-muted-foreground">
                            GOOGLE_CLOUD_LOCATION
                          </label>
                          <Input
                            value={selectedDraft.googleCloudLocation}
                            onChange={(event) => {
                              handleGeminiFieldChange(
                                "googleCloudLocation",
                                event.target.value
                              )
                            }}
                            placeholder="global / us-central1"
                          />
                        </div>
                      </div>
                    )}

                    {selectedDraft.geminiAuthMode ===
                      "vertex_service_account" && (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          GOOGLE_APPLICATION_CREDENTIALS
                        </label>
                        <Input
                          value={selectedDraft.googleApplicationCredentials}
                          onChange={(event) => {
                            handleGeminiFieldChange(
                              "googleApplicationCredentials",
                              event.target.value
                            )
                          }}
                          placeholder="/path/to/service-account.json"
                        />
                      </div>
                    )}

                    <div className="flex items-center justify-between gap-2">
                      <Button
                        type="button"
                        size="sm"
                        variant="outline"
                        onClick={() => {
                          openUrl(
                            "https://geminicli.com/docs/get-started/authentication/"
                          ).catch((err) => {
                            console.error(
                              "[Settings] open gemini auth doc failed:",
                              err
                            )
                          })
                        }}
                      >
                        {t("gemini.viewAuthDoc")}
                      </Button>
                      <Button
                        size="sm"
                        onClick={() => {
                          if (selectedMissingModelProvider) {
                            toast.error(t("toasts.modelProviderRequired"))
                            return
                          }
                          Promise.all([
                            persistEnv(
                              selectedAgent.agent_type,
                              selectedDraft.enabled,
                              selectedDraft.envText,
                              selectedDraft.modelProviderId
                            ),
                            persistConfig(
                              selectedAgent.agent_type,
                              selectedDraft.configText
                            ),
                          ])
                            .then(() => {
                              toast.success(t("toasts.geminiSaved"), {
                                description: t("toasts.configSavedHint"),
                              })
                            })
                            .catch((err) => {
                              console.error(
                                "[Settings] save gemini config failed:",
                                err
                              )
                              const message =
                                err instanceof Error ? err.message : String(err)
                              toast.error(t("toasts.saveGeminiFailed"), {
                                description: message,
                              })
                            })
                        }}
                        disabled={selectedIsSavingEnv || selectedIsSavingConfig}
                      >
                        {selectedIsSavingEnv || selectedIsSavingConfig ? (
                          <>
                            <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            {t("actions.saving")}
                          </>
                        ) : (
                          <>
                            <Save className="h-3.5 w-3.5" />
                            {t("actions.saveGeminiConfig")}
                          </>
                        )}
                      </Button>
                    </div>
                  </div>
                ) : selectedAgent.agent_type === "open_code" ? (
                  <div className="space-y-3 rounded-md border bg-muted/10 p-3">
                    <div>
                      <label className="text-xs font-medium">
                        {t("openCode.configManagement")}
                      </label>
                      <p className="mt-1 text-[11px] text-muted-foreground">
                        {t("openCode.configDescription")}
                      </p>
                    </div>

                    <div className="grid gap-3 md:grid-cols-2">
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          {t("openCode.mainModel")}
                        </label>
                        <OpenCodeModelCombobox
                          value={selectedOpenCodeConfig?.model ?? ""}
                          onValueChange={(v) =>
                            handleOpenCodeFieldChange("model", v)
                          }
                          groups={openCodeModelOptions}
                          placeholder="provider/model-id"
                        />
                      </div>
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          {t("openCode.smallModel")}
                        </label>
                        <OpenCodeModelCombobox
                          value={selectedOpenCodeConfig?.smallModel ?? ""}
                          onValueChange={(v) =>
                            handleOpenCodeFieldChange("small_model", v)
                          }
                          groups={openCodeModelOptions}
                          placeholder="provider/model-id"
                        />
                      </div>
                    </div>

                    <div className="space-y-2 rounded-md border bg-background/60 p-3">
                      <div className="flex items-center justify-between gap-2">
                        <label className="text-[11px] font-medium">
                          {t("openCode.providerManagement")}
                        </label>
                        <div className="text-[11px] text-muted-foreground">
                          {t("openCode.providerCount", {
                            count:
                              selectedOpenCodeConfig?.providerIds.length ?? 0,
                          })}
                        </div>
                      </div>

                      <div className="flex flex-wrap gap-2">
                        <Input
                          value={openCodeNewProviderId}
                          onChange={(event) => {
                            setOpenCodeNewProviderId(event.target.value)
                          }}
                          className="w-[220px]"
                          placeholder="new-provider-id"
                        />
                        <Button
                          type="button"
                          size="sm"
                          variant="outline"
                          onClick={handleOpenCodeAddProvider}
                        >
                          {t("openCode.addProvider")}
                        </Button>
                      </div>

                      {(selectedOpenCodeConfig?.providerIds.length ?? 0) ===
                      0 ? (
                        <div className="text-[11px] text-muted-foreground">
                          {t("openCode.emptyProvider")}
                        </div>
                      ) : (
                        <div className="space-y-2">
                          {selectedOpenCodeConfig?.providerIds.map(
                            (providerId) => {
                              const provider =
                                selectedOpenCodeConfig.providers[providerId]
                              if (!provider) return null
                              const expanded = openCodeProviderId === providerId
                              const isDisabled =
                                selectedOpenCodeConfig.disabledProviders.includes(
                                  providerId
                                )
                              return (
                                <Collapsible
                                  key={providerId}
                                  open={expanded}
                                  onOpenChange={(open) => {
                                    setOpenCodeProviderId(
                                      open ? providerId : ""
                                    )
                                  }}
                                >
                                  <div className="rounded-md border bg-muted/20">
                                    <div className="flex items-center justify-between gap-2 px-2.5 py-2">
                                      <button
                                        type="button"
                                        className="flex min-w-0 flex-1 items-center gap-2 text-left"
                                        onClick={() => {
                                          setOpenCodeProviderId((current) =>
                                            current === providerId
                                              ? ""
                                              : providerId
                                          )
                                        }}
                                      >
                                        <ChevronDown
                                          className={cn(
                                            "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform",
                                            expanded && "rotate-180"
                                          )}
                                        />
                                        <span className="truncate text-xs font-medium">
                                          {providerId}
                                        </span>
                                        <span className="text-[11px] text-muted-foreground">
                                          models: {provider.modelCount}
                                        </span>
                                      </button>
                                      <div className="flex items-center gap-3">
                                        <span className="text-[11px] text-muted-foreground">
                                          {isDisabled
                                            ? t("status.disabled")
                                            : t("status.enabled")}
                                        </span>
                                        <Switch
                                          checked={!isDisabled}
                                          onCheckedChange={(checked) => {
                                            handleOpenCodeProviderStatusChange(
                                              providerId,
                                              checked
                                            )
                                          }}
                                          aria-label={t(
                                            "openCode.providerEnabledState",
                                            { providerId }
                                          )}
                                          title={
                                            isDisabled
                                              ? t("actions.clickEnable", {
                                                  name: providerId,
                                                })
                                              : t("actions.clickDisable", {
                                                  name: providerId,
                                                })
                                          }
                                        />
                                        <Button
                                          type="button"
                                          size="xs"
                                          variant="outline"
                                          onClick={() => {
                                            setOpenCodeDeleteProviderId(
                                              providerId
                                            )
                                          }}
                                        >
                                          {t("actions.delete")}
                                        </Button>
                                      </div>
                                    </div>

                                    <CollapsibleContent className="px-2.5 pb-2.5">
                                      <div className="grid gap-3 border-t pt-2.5 md:grid-cols-2">
                                        <div className="space-y-1.5">
                                          <label className="text-[11px] text-muted-foreground">
                                            provider.name
                                          </label>
                                          <Input
                                            value={provider.name}
                                            onChange={(event) => {
                                              handleOpenCodeProviderFieldChange(
                                                providerId,
                                                "name",
                                                event.target.value
                                              )
                                            }}
                                            placeholder="My Provider"
                                          />
                                        </div>
                                        <div className="space-y-1.5">
                                          <label className="text-[11px] text-muted-foreground">
                                            provider.npm
                                          </label>
                                          <Select
                                            value={
                                              provider.npm.trim()
                                                ? provider.npm
                                                : "__none__"
                                            }
                                            onValueChange={(value) => {
                                              handleOpenCodeProviderFieldChange(
                                                providerId,
                                                "npm",
                                                value === "__none__"
                                                  ? ""
                                                  : value
                                              )
                                            }}
                                          >
                                            <SelectTrigger className="w-full">
                                              <SelectValue
                                                placeholder={t(
                                                  "openCode.selectProviderNpm"
                                                )}
                                              />
                                            </SelectTrigger>
                                            <SelectContent align="start">
                                              <SelectItem value="__none__">
                                                {t("openCode.notSet")}
                                              </SelectItem>
                                              {buildOpenCodeNpmOptions(
                                                provider.npm
                                              ).map((npmOption) => (
                                                <SelectItem
                                                  key={npmOption}
                                                  value={npmOption}
                                                >
                                                  {npmOption}
                                                </SelectItem>
                                              ))}
                                            </SelectContent>
                                          </Select>
                                        </div>
                                        <div className="space-y-1.5">
                                          <label className="text-[11px] text-muted-foreground">
                                            provider.api
                                          </label>
                                          <Input
                                            value={provider.api}
                                            onChange={(event) => {
                                              handleOpenCodeProviderFieldChange(
                                                providerId,
                                                "api",
                                                event.target.value
                                              )
                                            }}
                                            placeholder="openai.responses"
                                          />
                                        </div>
                                        <div className="space-y-1.5">
                                          <label className="text-[11px] text-muted-foreground">
                                            provider.options.baseURL
                                          </label>
                                          <Input
                                            value={provider.baseUrl}
                                            onChange={(event) => {
                                              handleOpenCodeProviderFieldChange(
                                                providerId,
                                                "baseURL",
                                                event.target.value
                                              )
                                            }}
                                            placeholder="https://api.example.com/v1"
                                          />
                                        </div>
                                        <div className="space-y-1.5 md:col-span-2">
                                          <label className="text-[11px] text-muted-foreground">
                                            provider.options.apiKey
                                          </label>
                                          <div className="flex items-center gap-2">
                                            <Input
                                              type={
                                                showApiKeys[
                                                  selectedAgent.agent_type
                                                ]
                                                  ? "text"
                                                  : "password"
                                              }
                                              value={provider.apiKey}
                                              onChange={(event) => {
                                                handleOpenCodeProviderFieldChange(
                                                  providerId,
                                                  "apiKey",
                                                  event.target.value
                                                )
                                              }}
                                              placeholder="sk-..."
                                            />
                                            <Button
                                              type="button"
                                              variant="outline"
                                              size="sm"
                                              onClick={() => {
                                                setShowApiKeys((prev) => ({
                                                  ...prev,
                                                  [selectedAgent.agent_type]:
                                                    !prev[
                                                      selectedAgent.agent_type
                                                    ],
                                                }))
                                              }}
                                              title={
                                                showApiKeys[
                                                  selectedAgent.agent_type
                                                ]
                                                  ? t("actions.hideKey")
                                                  : t("actions.showKey")
                                              }
                                            >
                                              {showApiKeys[
                                                selectedAgent.agent_type
                                              ] ? (
                                                <EyeOff className="h-3.5 w-3.5" />
                                              ) : (
                                                <Eye className="h-3.5 w-3.5" />
                                              )}
                                            </Button>
                                          </div>
                                        </div>
                                      </div>
                                      <Collapsible
                                        open={Boolean(
                                          openCodeModelConfigExpanded[
                                            providerId
                                          ]
                                        )}
                                        onOpenChange={(open) => {
                                          setOpenCodeModelConfigExpanded(
                                            (prev) => ({
                                              ...prev,
                                              [providerId]: open,
                                            })
                                          )
                                        }}
                                      >
                                        <div className="mt-3 rounded-md border bg-background/50 p-2.5">
                                          <button
                                            type="button"
                                            className="flex w-full items-center justify-between gap-2 text-left"
                                            onClick={() => {
                                              setOpenCodeModelConfigExpanded(
                                                (prev) => ({
                                                  ...prev,
                                                  [providerId]:
                                                    !prev[providerId],
                                                })
                                              )
                                            }}
                                          >
                                            <div className="flex items-center gap-2">
                                              <ChevronDown
                                                className={cn(
                                                  "h-3.5 w-3.5 shrink-0 text-muted-foreground transition-transform",
                                                  openCodeModelConfigExpanded[
                                                    providerId
                                                  ] && "rotate-180"
                                                )}
                                              />
                                              <span className="text-[11px] font-medium">
                                                {t("openCode.modelManagement")}
                                              </span>
                                            </div>
                                            <span className="text-[11px] text-muted-foreground">
                                              {t("openCode.modelCount", {
                                                count: provider.modelCount,
                                              })}
                                            </span>
                                          </button>
                                          <CollapsibleContent className="pt-2">
                                            <p className="text-[11px] text-muted-foreground">
                                              {t("openCode.modelDescription")}
                                            </p>

                                            <div className="mt-2 flex flex-wrap items-center gap-2">
                                              <Input
                                                value={
                                                  openCodeNewModelIds[
                                                    providerId
                                                  ] ?? ""
                                                }
                                                onChange={(event) => {
                                                  handleOpenCodeModelDraftChange(
                                                    providerId,
                                                    event.target.value
                                                  )
                                                }}
                                                className="w-[240px]"
                                                placeholder="new-model-id"
                                              />
                                              <Button
                                                type="button"
                                                size="sm"
                                                variant="outline"
                                                onClick={() => {
                                                  handleOpenCodeAddModel(
                                                    providerId
                                                  )
                                                }}
                                              >
                                                {t("openCode.addModel")}
                                              </Button>
                                            </div>

                                            {provider.modelIds.length === 0 ? (
                                              <div className="mt-2 text-[11px] text-muted-foreground">
                                                {t("openCode.emptyModel")}
                                              </div>
                                            ) : (
                                              <div className="mt-2 space-y-1">
                                                <div className="flex items-center gap-2 px-1 text-[10px] text-muted-foreground">
                                                  <div className="min-w-0 flex-1">
                                                    {t("openCode.modelId")}
                                                  </div>
                                                  <div className="min-w-0 flex-1">
                                                    {t("openCode.modelName")}
                                                  </div>
                                                  <div className="size-8 shrink-0" />
                                                </div>
                                                {provider.modelIds.map(
                                                  (modelId) => {
                                                    const model =
                                                      provider.models[modelId]
                                                    if (!model) return null
                                                    const modelDraftKey = `${providerId}:${modelId}`
                                                    return (
                                                      <div
                                                        key={`${providerId}:${modelId}`}
                                                        className="flex items-center gap-2"
                                                      >
                                                        <Input
                                                          value={
                                                            openCodeModelIdDrafts[
                                                              modelDraftKey
                                                            ] ?? model.id
                                                          }
                                                          onChange={(event) => {
                                                            handleOpenCodeModelIdDraftChange(
                                                              providerId,
                                                              modelId,
                                                              event.target.value
                                                            )
                                                          }}
                                                          onBlur={() => {
                                                            handleOpenCodeModelIdCommit(
                                                              providerId,
                                                              modelId
                                                            )
                                                          }}
                                                          onKeyDown={(
                                                            event
                                                          ) => {
                                                            if (
                                                              event.key ===
                                                              "Enter"
                                                            ) {
                                                              event.preventDefault()
                                                              handleOpenCodeModelIdCommit(
                                                                providerId,
                                                                modelId
                                                              )
                                                              event.currentTarget.blur()
                                                              return
                                                            }
                                                            if (
                                                              event.key ===
                                                              "Escape"
                                                            ) {
                                                              setOpenCodeModelIdDrafts(
                                                                (prev) => {
                                                                  if (
                                                                    typeof prev[
                                                                      modelDraftKey
                                                                    ] ===
                                                                    "undefined"
                                                                  ) {
                                                                    return prev
                                                                  }
                                                                  const next = {
                                                                    ...prev,
                                                                  }
                                                                  delete next[
                                                                    modelDraftKey
                                                                  ]
                                                                  return next
                                                                }
                                                              )
                                                              event.currentTarget.blur()
                                                            }
                                                          }}
                                                          className="h-8 min-w-0 flex-1"
                                                          placeholder="model.id"
                                                        />
                                                        <Input
                                                          value={model.name}
                                                          onChange={(event) => {
                                                            handleOpenCodeModelFieldChange(
                                                              providerId,
                                                              modelId,
                                                              event.target.value
                                                            )
                                                          }}
                                                          className="h-8 min-w-0 flex-1"
                                                          placeholder="model.name"
                                                        />
                                                        <Button
                                                          type="button"
                                                          size="icon-sm"
                                                          variant="ghost"
                                                          className="shrink-0 text-muted-foreground hover:text-destructive"
                                                          aria-label={t(
                                                            "openCode.deleteModel",
                                                            { modelId }
                                                          )}
                                                          title={t(
                                                            "openCode.deleteModel",
                                                            { modelId }
                                                          )}
                                                          onClick={() => {
                                                            handleOpenCodeRemoveModel(
                                                              providerId,
                                                              modelId
                                                            )
                                                          }}
                                                        >
                                                          <Minus className="h-3.5 w-3.5" />
                                                        </Button>
                                                      </div>
                                                    )
                                                  }
                                                )}
                                              </div>
                                            )}
                                          </CollapsibleContent>
                                        </div>
                                      </Collapsible>
                                      <div className="mt-3 flex justify-end">
                                        <Button
                                          type="button"
                                          size="sm"
                                          onClick={() => {
                                            persistConfig(
                                              selectedAgent.agent_type,
                                              selectedDraft.configText,
                                              {
                                                openCodeAuthJsonText:
                                                  selectedDraft.openCodeAuthJsonText,
                                              }
                                            )
                                              .then(() => {
                                                toast.success(
                                                  t("toasts.providerSaved", {
                                                    providerId,
                                                  }),
                                                  {
                                                    description: `${t("toasts.openCodeConfigSynced")} ${t("toasts.configSavedHint")}`,
                                                  }
                                                )
                                              })
                                              .catch((err) => {
                                                console.error(
                                                  "[Settings] save opencode provider failed:",
                                                  err
                                                )
                                                const message =
                                                  err instanceof Error
                                                    ? err.message
                                                    : String(err)
                                                toast.error(
                                                  t(
                                                    "toasts.saveProviderFailed",
                                                    {
                                                      providerId,
                                                    }
                                                  ),
                                                  {
                                                    description: message,
                                                  }
                                                )
                                              })
                                          }}
                                          disabled={selectedIsSavingConfig}
                                        >
                                          {selectedIsSavingConfig ? (
                                            <>
                                              <Loader2 className="h-3.5 w-3.5 animate-spin" />
                                              {t("actions.saving")}
                                            </>
                                          ) : (
                                            <>
                                              <Save className="h-3.5 w-3.5" />
                                              {t("actions.saveCurrentProvider")}
                                            </>
                                          )}
                                        </Button>
                                      </div>
                                    </CollapsibleContent>
                                  </div>
                                </Collapsible>
                              )
                            }
                          )}
                        </div>
                      )}
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        {t("openCode.nativeJsonConfig")}
                      </label>
                      <Textarea
                        value={selectedDraft.configText}
                        onChange={(event) => {
                          handleConfigTextChange(event.target.value)
                        }}
                        placeholder={`{
  "$schema": "https://opencode.ai/config.json",
  "model": "google/gemini-3-pro-preview",
  "provider": {
    "google": {
      "options": {
        "baseURL": "https://generativelanguage.googleapis.com/v1beta"
      }
    }
  }
}`}
                        className="min-h-44 max-h-96 overflow-y-auto font-mono text-xs"
                      />
                      {selectedConfigError && (
                        <div className="rounded-md border border-red-500/30 bg-red-500/5 px-2.5 py-1.5 text-[11px] text-red-400">
                          {selectedConfigError}
                        </div>
                      )}
                    </div>

                    <div className="flex justify-end">
                      <Button
                        size="sm"
                        onClick={() => {
                          persistConfig(
                            selectedAgent.agent_type,
                            selectedDraft.configText,
                            {
                              openCodeAuthJsonText:
                                selectedDraft.openCodeAuthJsonText,
                            }
                          )
                            .then(() => {
                              toast.success(t("toasts.openCodeSaved"), {
                                description: t("toasts.configSavedHint"),
                              })
                            })
                            .catch((err) => {
                              console.error(
                                "[Settings] save opencode config failed:",
                                err
                              )
                              const message =
                                err instanceof Error ? err.message : String(err)
                              toast.error(t("toasts.saveOpenCodeFailed"), {
                                description: message,
                              })
                            })
                        }}
                        disabled={selectedIsSavingConfig}
                      >
                        {selectedIsSavingConfig ? (
                          <>
                            <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            {t("actions.saving")}
                          </>
                        ) : (
                          <>
                            <Save className="h-3.5 w-3.5" />
                            {t("actions.saveOpenCodeConfig")}
                          </>
                        )}
                      </Button>
                    </div>
                  </div>
                ) : selectedAgent.agent_type === "cline" ? (
                  <div className="space-y-3 rounded-md border bg-muted/10 p-3">
                    <div>
                      <label className="text-xs font-medium">Cline</label>
                      <p className="mt-1 text-[11px] text-muted-foreground">
                        {t("cline.configDescription")}
                      </p>
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        Provider
                      </label>
                      <Select
                        value={selectedDraft.clineProvider}
                        onValueChange={(value) => {
                          handleClineFieldChange("clineProvider", value)
                        }}
                      >
                        <SelectTrigger className="h-8 text-xs">
                          <SelectValue />
                        </SelectTrigger>
                        <SelectContent>
                          {CLINE_PROVIDERS.map((p) => (
                            <SelectItem key={p.value} value={p.value}>
                              {p.label}
                            </SelectItem>
                          ))}
                        </SelectContent>
                      </Select>
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        API Key
                      </label>
                      <div className="flex items-center gap-2">
                        <Input
                          type={
                            showApiKeys[selectedAgent.agent_type]
                              ? "text"
                              : "password"
                          }
                          value={selectedDraft.clineApiKey}
                          onChange={(event) => {
                            handleClineFieldChange(
                              "clineApiKey",
                              event.target.value
                            )
                          }}
                          placeholder="sk-..."
                        />
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={() => {
                            setShowApiKeys((prev) => ({
                              ...prev,
                              [selectedAgent.agent_type]:
                                !prev[selectedAgent.agent_type],
                            }))
                          }}
                          title={
                            showApiKeys[selectedAgent.agent_type]
                              ? t("actions.hideApiKey")
                              : t("actions.showApiKey")
                          }
                        >
                          {showApiKeys[selectedAgent.agent_type] ? (
                            <EyeOff className="h-3.5 w-3.5" />
                          ) : (
                            <Eye className="h-3.5 w-3.5" />
                          )}
                        </Button>
                      </div>
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        Model
                      </label>
                      <Input
                        value={selectedDraft.clineModel}
                        onChange={(event) => {
                          handleClineFieldChange(
                            "clineModel",
                            event.target.value
                          )
                        }}
                        placeholder="claude-sonnet-4-5-20250514"
                      />
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        API URL
                      </label>
                      <Input
                        value={selectedDraft.clineBaseUrl}
                        onChange={(event) => {
                          handleClineFieldChange(
                            "clineBaseUrl",
                            event.target.value
                          )
                        }}
                        placeholder="https://api.openai.com"
                      />
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        {t("nativeJsonConfig")} (config)
                      </label>
                      <Textarea
                        value={selectedDraft.configText}
                        onChange={(event) => {
                          handleConfigTextChange(event.target.value)
                        }}
                        className="min-h-24 font-mono text-xs"
                        placeholder={`{
  "apiProvider": "anthropic",
  "apiKey": "sk-...",
  "model": "claude-sonnet-4-5-20250514"
}`}
                      />
                      {selectedConfigError && (
                        <div className="rounded-md border border-red-500/30 bg-red-500/5 px-2.5 py-1.5 text-[11px] text-red-400">
                          {selectedConfigError}
                        </div>
                      )}
                    </div>

                    <div className="flex items-center justify-end gap-2">
                      <Button
                        size="sm"
                        onClick={() => {
                          persistConfig(
                            selectedAgent.agent_type,
                            selectedDraft.configText
                          )
                            .then(() => {
                              toast.success(t("toasts.clineSaved"), {
                                description: t("toasts.configSavedHint"),
                              })
                            })
                            .catch((err) => {
                              console.error(
                                "[Settings] save cline config failed:",
                                err
                              )
                              const message =
                                err instanceof Error ? err.message : String(err)
                              toast.error(t("toasts.saveClineFailed"), {
                                description: message,
                              })
                            })
                        }}
                        disabled={selectedIsSavingConfig}
                      >
                        {selectedIsSavingConfig ? (
                          <>
                            <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            {t("actions.saving")}
                          </>
                        ) : (
                          <>
                            <Save className="h-3.5 w-3.5" />
                            {t("actions.saveClineConfig")}
                          </>
                        )}
                      </Button>
                    </div>
                  </div>
                ) : selectedAgent.agent_type === "open_claw" ? (
                  <div className="space-y-3 rounded-md border bg-muted/10 p-3">
                    <div>
                      <label className="text-xs font-medium">
                        {t("openClaw.gatewayConfig")}
                      </label>
                      <p className="mt-1 text-[11px] text-muted-foreground">
                        {t("openClaw.gatewayDescription")}
                      </p>
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        Gateway URL
                      </label>
                      <Input
                        value={selectedDraft.openClawGatewayUrl}
                        onChange={(event) => {
                          handleOpenClawFieldChange(
                            "openClawGatewayUrl",
                            event.target.value
                          )
                        }}
                        placeholder="wss://gateway-host:18789"
                      />
                      <p className="text-[11px] text-muted-foreground">
                        {t("openClaw.gatewayUrlHint")}
                      </p>
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        Gateway Token
                      </label>
                      <div className="flex items-center gap-2">
                        <Input
                          type={
                            showApiKeys[selectedAgent.agent_type]
                              ? "text"
                              : "password"
                          }
                          value={selectedDraft.openClawGatewayToken}
                          onChange={(event) => {
                            handleOpenClawFieldChange(
                              "openClawGatewayToken",
                              event.target.value
                            )
                          }}
                          placeholder={t("openClaw.gatewayTokenPlaceholder")}
                        />
                        <Button
                          type="button"
                          variant="outline"
                          size="sm"
                          onClick={() => {
                            setShowApiKeys((prev) => ({
                              ...prev,
                              [selectedAgent.agent_type]:
                                !prev[selectedAgent.agent_type],
                            }))
                          }}
                          title={
                            showApiKeys[selectedAgent.agent_type]
                              ? t("actions.hideToken")
                              : t("actions.showToken")
                          }
                        >
                          {showApiKeys[selectedAgent.agent_type] ? (
                            <EyeOff className="h-3.5 w-3.5" />
                          ) : (
                            <Eye className="h-3.5 w-3.5" />
                          )}
                        </Button>
                      </div>
                      <p className="text-[11px] text-muted-foreground">
                        {t("openClaw.gatewayTokenHint")}
                      </p>
                    </div>

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        Session Key
                      </label>
                      <Input
                        value={selectedDraft.openClawSessionKey}
                        onChange={(event) => {
                          handleOpenClawFieldChange(
                            "openClawSessionKey",
                            event.target.value
                          )
                        }}
                        placeholder="agent:main:main"
                      />
                      <p className="text-[11px] text-muted-foreground">
                        {t("openClaw.sessionKeyHint")}
                      </p>
                    </div>

                    <div className="flex items-center justify-end gap-2">
                      <Button
                        size="sm"
                        onClick={() => {
                          Promise.all([
                            persistEnv(
                              selectedAgent.agent_type,
                              selectedDraft.enabled,
                              selectedDraft.envText,
                              selectedDraft.modelProviderId
                            ),
                            persistConfig(
                              selectedAgent.agent_type,
                              selectedDraft.configText
                            ),
                          ])
                            .then(() => {
                              toast.success(t("toasts.openClawSaved"), {
                                description: t("toasts.configSavedHint"),
                              })
                            })
                            .catch((err) => {
                              console.error(
                                "[Settings] save openclaw config failed:",
                                err
                              )
                              const message =
                                err instanceof Error ? err.message : String(err)
                              toast.error(t("toasts.saveOpenClawFailed"), {
                                description: message,
                              })
                            })
                        }}
                        disabled={selectedIsSavingEnv || selectedIsSavingConfig}
                      >
                        {selectedIsSavingEnv || selectedIsSavingConfig ? (
                          <>
                            <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            {t("actions.saving")}
                          </>
                        ) : (
                          <>
                            <Save className="h-3.5 w-3.5" />
                            {t("actions.saveOpenClawConfig")}
                          </>
                        )}
                      </Button>
                    </div>
                  </div>
                ) : (
                  <div className="space-y-3 rounded-md border bg-muted/10 p-3">
                    <div>
                      <label className="text-xs font-medium">
                        {t("configManagement")}
                      </label>
                      <p className="mt-1 text-[11px] text-muted-foreground">
                        {selectedAgent.agent_type === "claude_code"
                          ? t("generalConfigDescriptionClaude")
                          : t("generalConfigDescriptionDefault")}
                      </p>
                    </div>

                    {selectedAgent.agent_type === "claude_code" && (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          {t("claude.authMode")}
                        </label>
                        <Select
                          value={selectedDraft.claudeAuthMode}
                          onValueChange={(value) => {
                            if (
                              CLAUDE_AUTH_MODES.includes(
                                value as ClaudeAuthMode
                              )
                            ) {
                              handleClaudeAuthModeChange(
                                value as ClaudeAuthMode
                              )
                            }
                          }}
                        >
                          <SelectTrigger className="w-full">
                            <SelectValue />
                          </SelectTrigger>
                          <SelectContent align="start">
                            <SelectItem value="official_subscription">
                              {t("authModeOfficialSubscription")}
                            </SelectItem>
                            <SelectItem value="custom">
                              {t("authModeCustomEndpoint")}
                            </SelectItem>
                            <SelectItem value="model_provider">
                              {t("authModeModelProvider")}
                            </SelectItem>
                          </SelectContent>
                        </Select>
                        <p className="text-[11px] text-muted-foreground">
                          {selectedDraft.claudeAuthMode ===
                          "official_subscription"
                            ? t("claude.officialSubscriptionHint")
                            : selectedDraft.claudeAuthMode === "custom"
                              ? t("authModeCustomEndpointHint")
                              : t("modelProviderHint")}
                        </p>
                      </div>
                    )}

                    {selectedAgent.agent_type === "claude_code" &&
                      selectedDraft.claudeAuthMode === "model_provider" && (
                        <div className="space-y-1.5">
                          <label className="text-[11px] text-muted-foreground">
                            {t("selectModelProvider")}
                          </label>
                          {selectedModelProviders.length > 0 ? (
                            <Select
                              value={
                                selectedDraft.modelProviderId != null
                                  ? String(selectedDraft.modelProviderId)
                                  : ""
                              }
                              onValueChange={handleModelProviderSelect}
                            >
                              <SelectTrigger className="w-full">
                                <SelectValue
                                  placeholder={t("selectModelProvider")}
                                />
                              </SelectTrigger>
                              <SelectContent align="start">
                                {selectedModelProviders.map((provider) => (
                                  <SelectItem
                                    key={provider.id}
                                    value={String(provider.id)}
                                  >
                                    {provider.name}
                                  </SelectItem>
                                ))}
                              </SelectContent>
                            </Select>
                          ) : (
                            <p className="text-[11px] text-muted-foreground">
                              {t("noModelProviderAvailable")}
                            </p>
                          )}
                        </div>
                      )}

                    {(selectedAgent.agent_type !== "claude_code" ||
                      selectedDraft.claudeAuthMode === "custom") && (
                      <>
                        <div className="space-y-1.5">
                          <label className="text-[11px] text-muted-foreground">
                            API URL
                          </label>
                          <Input
                            value={selectedDraft.apiBaseUrl}
                            readOnly={
                              selectedAgent.agent_type === "claude_code" &&
                              selectedDraft.claudeAuthMode === "model_provider"
                            }
                            onChange={(event) => {
                              handleImportantConfigChange(
                                "apiBaseUrl",
                                event.target.value
                              )
                            }}
                            placeholder="https://api.example.com"
                          />
                        </div>

                        <div className="space-y-1.5">
                          <label className="text-[11px] text-muted-foreground">
                            API Key
                          </label>
                          <div className="flex items-center gap-2">
                            <Input
                              type={
                                showApiKeys[selectedAgent.agent_type]
                                  ? "text"
                                  : "password"
                              }
                              value={selectedDraft.apiKey}
                              readOnly={
                                selectedAgent.agent_type === "claude_code" &&
                                selectedDraft.claudeAuthMode ===
                                  "model_provider"
                              }
                              onChange={(event) => {
                                handleImportantConfigChange(
                                  "apiKey",
                                  event.target.value
                                )
                              }}
                              placeholder="sk-..."
                            />
                            <Button
                              type="button"
                              variant="outline"
                              size="sm"
                              onClick={() => {
                                setShowApiKeys((prev) => ({
                                  ...prev,
                                  [selectedAgent.agent_type]:
                                    !prev[selectedAgent.agent_type],
                                }))
                              }}
                              title={
                                showApiKeys[selectedAgent.agent_type]
                                  ? t("actions.hideApiKey")
                                  : t("actions.showApiKey")
                              }
                            >
                              {showApiKeys[selectedAgent.agent_type] ? (
                                <EyeOff className="h-3.5 w-3.5" />
                              ) : (
                                <Eye className="h-3.5 w-3.5" />
                              )}
                            </Button>
                          </div>
                        </div>
                      </>
                    )}

                    {selectedAgent.agent_type === "claude_code" ? (
                      <div className="space-y-2">
                        <div className="grid gap-3 md:grid-cols-2">
                          <div className="space-y-1.5">
                            <label className="text-[11px] text-muted-foreground">
                              {t("claude.mainModel")}
                            </label>
                            <Input
                              value={selectedDraft.claudeMainModel}
                              onChange={(event) => {
                                handleImportantConfigChange(
                                  "claudeMainModel",
                                  event.target.value
                                )
                              }}
                              placeholder="claude-sonnet-4-6"
                            />
                          </div>
                          <div className="space-y-1.5">
                            <label className="text-[11px] text-muted-foreground">
                              {t("claude.reasoningModel")}
                            </label>
                            <Input
                              value={selectedDraft.claudeReasoningModel}
                              onChange={(event) => {
                                handleImportantConfigChange(
                                  "claudeReasoningModel",
                                  event.target.value
                                )
                              }}
                              placeholder="claude-opus-4-6"
                            />
                          </div>
                          <div className="space-y-1.5">
                            <label className="text-[11px] text-muted-foreground">
                              {t("claude.haikuDefaultModel")}
                            </label>
                            <Input
                              value={selectedDraft.claudeDefaultHaikuModel}
                              onChange={(event) => {
                                handleImportantConfigChange(
                                  "claudeDefaultHaikuModel",
                                  event.target.value
                                )
                              }}
                              placeholder="claude-haiku-4-5-20251001"
                            />
                          </div>
                          <div className="space-y-1.5">
                            <label className="text-[11px] text-muted-foreground">
                              {t("claude.sonnetDefaultModel")}
                            </label>
                            <Input
                              value={selectedDraft.claudeDefaultSonnetModel}
                              onChange={(event) => {
                                handleImportantConfigChange(
                                  "claudeDefaultSonnetModel",
                                  event.target.value
                                )
                              }}
                              placeholder="claude-sonnet-4-6"
                            />
                          </div>
                          <div className="space-y-1.5 md:col-span-2">
                            <label className="text-[11px] text-muted-foreground">
                              {t("claude.opusDefaultModel")}
                            </label>
                            <Input
                              value={selectedDraft.claudeDefaultOpusModel}
                              onChange={(event) => {
                                handleImportantConfigChange(
                                  "claudeDefaultOpusModel",
                                  event.target.value
                                )
                              }}
                              placeholder="claude-opus-4-6"
                            />
                          </div>
                        </div>
                        <p className="text-[11px] text-muted-foreground">
                          {t("modelHintDefault")}
                        </p>
                      </div>
                    ) : (
                      <div className="space-y-1.5">
                        <label className="text-[11px] text-muted-foreground">
                          Model
                        </label>
                        <Input
                          value={selectedDraft.model}
                          onChange={(event) => {
                            handleImportantConfigChange(
                              "model",
                              event.target.value
                            )
                          }}
                          placeholder="gpt-5 / claude-sonnet / gemini-2.5-pro"
                        />
                      </div>
                    )}

                    <div className="space-y-1.5">
                      <label className="text-[11px] text-muted-foreground">
                        {t("nativeJsonConfig")}
                      </label>
                      <Textarea
                        value={selectedDraft.configText}
                        onChange={(event) => {
                          handleConfigTextChange(event.target.value)
                        }}
                        placeholder={`{
  "apiBaseUrl": "https://api.example.com",
  "apiKey": "sk-...",
  "model": "gpt-5",
  "env": {
    "CUSTOM_KEY": "VALUE"
  }
}`}
                        className="min-h-36 font-mono text-xs"
                      />
                      {selectedConfigError && (
                        <div className="rounded-md border border-red-500/30 bg-red-500/5 px-2.5 py-1.5 text-[11px] text-red-400">
                          {selectedConfigError}
                        </div>
                      )}
                    </div>

                    <div className="flex justify-end">
                      <Button
                        size="sm"
                        onClick={() => {
                          if (selectedMissingModelProvider) {
                            toast.error(t("toasts.modelProviderRequired"))
                            return
                          }
                          Promise.all([
                            persistEnv(
                              selectedAgent.agent_type,
                              selectedDraft.enabled,
                              selectedDraft.envText,
                              selectedDraft.modelProviderId
                            ),
                            persistConfig(
                              selectedAgent.agent_type,
                              selectedDraft.configText
                            ),
                          ])
                            .then(() => {
                              toast.success(t("toasts.configSaved"), {
                                description: t("toasts.configSavedHint"),
                              })
                            })
                            .catch((err) => {
                              console.error(
                                "[Settings] save config management failed:",
                                err
                              )
                              const message =
                                err instanceof Error ? err.message : String(err)
                              toast.error(
                                t("toasts.saveConfigManagementFailed"),
                                {
                                  description: message,
                                }
                              )
                            })
                        }}
                        disabled={selectedIsSavingEnv || selectedIsSavingConfig}
                      >
                        {selectedIsSavingEnv || selectedIsSavingConfig ? (
                          <>
                            <Loader2 className="h-3.5 w-3.5 animate-spin" />
                            {t("actions.saving")}
                          </>
                        ) : (
                          <>
                            <Save className="h-3.5 w-3.5" />
                            {t("actions.saveConfigManagement")}
                          </>
                        )}
                      </Button>
                    </div>
                  </div>
                )}
              </div>
            </div>
          ) : (
            <div className="h-full flex items-center justify-center text-xs text-muted-foreground">
              {t("emptyNoAgent")}
            </div>
          )}
        </div>
      </div>

      <AlertDialog
        open={Boolean(openCodeDeleteProviderId)}
        onOpenChange={(open) => {
          if (!open) setOpenCodeDeleteProviderId(null)
        }}
      >
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("dialogs.confirmDeleteProvider", {
                providerId: openCodeDeleteProviderId ?? "",
              })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("dialogs.confirmDeleteProviderDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel disabled={selectedIsSaving}>
              {t("actions.cancel")}
            </AlertDialogCancel>
            <Button
              variant="destructive"
              onClick={confirmOpenCodeProviderDelete}
              disabled={selectedIsSaving}
            >
              {selectedIsSaving ? (
                <>
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  {t("actions.deleting")}
                </>
              ) : (
                <>
                  <Trash2 className="h-3.5 w-3.5" />
                  {t("actions.confirmDelete")}
                </>
              )}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>

      <AlertDialog
        open={Boolean(uninstallConfirmAgent)}
        onOpenChange={(open) => {
          if (!open) setUninstallConfirmAgent(null)
        }}
      >
        <AlertDialogContent size="sm">
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("dialogs.confirmUninstall", {
                name: uninstallConfirmAgent?.name ?? "Agent",
              })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("dialogs.confirmUninstallDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel
              disabled={
                uninstallConfirmAgent
                  ? Boolean(busyBinaryAction[uninstallConfirmAgent.agent_type])
                  : false
              }
            >
              {t("actions.cancel")}
            </AlertDialogCancel>
            <Button
              variant="destructive"
              onClick={confirmUninstall}
              disabled={
                uninstallConfirmAgent
                  ? Boolean(busyBinaryAction[uninstallConfirmAgent.agent_type])
                  : false
              }
            >
              {uninstallConfirmAgent &&
              busyBinaryAction[uninstallConfirmAgent.agent_type] ? (
                <>
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  {t("actions.uninstalling")}
                </>
              ) : (
                <>
                  <Trash2 className="h-3.5 w-3.5" />
                  {t("actions.confirmUninstall")}
                </>
              )}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
