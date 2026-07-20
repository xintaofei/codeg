"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import {
  Globe,
  Loader2,
  Plus,
  RefreshCw,
  Search,
  ShieldCheck,
  TerminalSquare,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuTrigger,
} from "@/components/ui/context-menu"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Textarea } from "@/components/ui/textarea"
import {
  mcpGetMarketplaceServerDetail,
  mcpInstallFromMarketplace,
  mcpListMarketplaces,
  mcpRemoveServer,
  mcpScanLocal,
  mcpSearchMarketplace,
  mcpUpsertLocalServer,
} from "@/lib/api"
import { toLocalizedErrorMessage } from "@/lib/app-error"
import { normalizeMcpType } from "@/lib/mcp-types"
import { cn } from "@/lib/utils"
import type {
  LocalMcpServer,
  McpAppType,
  McpMarketplaceItem,
  McpMarketplaceInstallOption,
  McpMarketplaceProvider,
  McpMarketplaceServerDetail,
} from "@/lib/types"

type LeftTab = "local" | "market"

type Selection =
  | { kind: "local"; id: string }
  | { kind: "market"; id: string }
  | { kind: "draft" }
  | null

const DEFAULT_DRAFT_SPEC = JSON.stringify(
  {
    type: "stdio",
    command: "",
    args: [],
  },
  null,
  2
)

type McpTranslator = (
  key: string,
  values?: Record<string, string | number>
) => string

const APP_OPTIONS: { value: McpAppType; label: string }[] = [
  { value: "claude_code", label: "Claude Code" },
  { value: "codex", label: "Codex CLI" },
  { value: "gemini", label: "Gemini CLI" },
  // OpenClaw 不接受 ACP 线缆上的 MCP 服务器条目（后端 registry.rs supports_mcp=false
  // 会让其 mcpServers 恒为空 []，否则带条目时 OpenClaw 会在建会话阶段报错），按产品
  // 决策不作为可分配目标。McpAppType 仍保留 "open_claw" 以兼容回读存量配置，
  // saveLocalServer 也会保留既有 open_claw 分配（不静默清除）。
  { value: "open_code", label: "OpenCode" },
  { value: "cline", label: "Cline" },
  { value: "hermes", label: "Hermes Agent" },
  { value: "code_buddy", label: "CodeBuddy" },
  { value: "kimi_code", label: "Kimi Code" },
  { value: "grok", label: "Grok" },
  { value: "cursor", label: "Cursor" },
]

function isObject(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value)
}

function readString(spec: Record<string, unknown>, key: string): string | null {
  const raw = spec[key]
  if (typeof raw !== "string") return null
  const trimmed = raw.trim()
  return trimmed ? trimmed : null
}

function specSummary(spec: Record<string, unknown>, t: McpTranslator): string {
  const typ = readString(spec, "type") ?? "stdio"

  if (typ === "stdio") {
    const command = readString(spec, "command") ?? t("summary.missingCommand")
    const rawArgs = spec.args
    const args = Array.isArray(rawArgs)
      ? rawArgs
          .map((item) => (typeof item === "string" ? item.trim() : ""))
          .filter(Boolean)
      : []
    return args.length > 0 ? `${command} ${args.join(" ")}` : command
  }

  const url = readString(spec, "url") ?? t("summary.missingUrl")
  return `${typ}: ${url}`
}

function protocolBadgeLabel(protocol: string, t: McpTranslator): string {
  const canonical = normalizeMcpType(protocol)
  if (canonical === "stdio") return t("protocol.stdio")
  if (canonical === "sse") return "SSE"
  if (canonical === "http") return "HTTP"
  return protocol
}

function defaultParamDraft(
  option: McpMarketplaceInstallOption | null
): Record<string, string> {
  if (!option) return {}
  const draft: Record<string, string> = {}
  for (const field of option.parameters) {
    if (field.default_value === null || field.default_value === undefined)
      continue
    if (typeof field.default_value === "string") {
      draft[field.key] = field.default_value
      continue
    }
    if (
      typeof field.default_value === "number" ||
      typeof field.default_value === "boolean"
    ) {
      draft[field.key] = String(field.default_value)
      continue
    }
    draft[field.key] = JSON.stringify(field.default_value)
  }
  return draft
}

function parseParameterValues(
  option: McpMarketplaceInstallOption | null,
  draft: Record<string, string>,
  t: McpTranslator
): { values: Record<string, unknown>; error: string | null } {
  if (!option) return { values: {}, error: t("errors.selectInstallProtocol") }

  const values: Record<string, unknown> = {}
  for (const field of option.parameters) {
    const raw = (draft[field.key] ?? "").trim()

    if (!raw) {
      if (field.required && field.default_value == null) {
        return {
          values: {},
          error: t("errors.fieldRequired", { field: field.label }),
        }
      }
      continue
    }

    if (field.kind === "boolean") {
      if (raw !== "true" && raw !== "false") {
        return {
          values: {},
          error: t("errors.fieldNeedsBoolean", { field: field.label }),
        }
      }
      values[field.key] = raw === "true"
      continue
    }

    if (field.kind === "number") {
      const next = Number(raw)
      if (!Number.isFinite(next)) {
        return {
          values: {},
          error: t("errors.fieldNeedsNumber", { field: field.label }),
        }
      }
      values[field.key] = next
      continue
    }

    if (field.kind === "integer") {
      const next = Number(raw)
      if (!Number.isInteger(next)) {
        return {
          values: {},
          error: t("errors.fieldNeedsInteger", { field: field.label }),
        }
      }
      values[field.key] = next
      continue
    }

    if (field.kind === "json") {
      try {
        values[field.key] = JSON.parse(raw)
      } catch (err) {
        const message = toLocalizedErrorMessage(err, t)
        return {
          values: {},
          error: t("errors.fieldInvalidJson", {
            field: field.label,
            message,
          }),
        }
      }
      continue
    }

    if (field.enum_values.length > 0 && !field.enum_values.includes(raw)) {
      return {
        values: {},
        error: t("errors.fieldOutOfRange", { field: field.label }),
      }
    }

    values[field.key] = raw
  }

  return { values, error: null }
}

function normalizeApps(apps: McpAppType[]): McpAppType[] {
  return [...new Set(apps)]
}

function appsToDraft(apps: McpAppType[]): Record<McpAppType, boolean> {
  const appSet = new Set(apps)
  return {
    claude_code: appSet.has("claude_code"),
    codex: appSet.has("codex"),
    gemini: appSet.has("gemini"),
    open_claw: appSet.has("open_claw"),
    open_code: appSet.has("open_code"),
    cline: appSet.has("cline"),
    hermes: appSet.has("hermes"),
    code_buddy: appSet.has("code_buddy"),
    kimi_code: appSet.has("kimi_code"),
    grok: appSet.has("grok"),
    cursor: appSet.has("cursor"),
  }
}

function selectedAppsFromDraft(
  draft: Record<McpAppType, boolean>
): McpAppType[] {
  return APP_OPTIONS.filter((item) => draft[item.value]).map(
    (item) => item.value
  )
}

function detectEnvOnRemote(text: string): boolean {
  const trimmed = text.trim()
  if (!trimmed) return false
  let parsed: unknown
  try {
    parsed = JSON.parse(trimmed)
  } catch {
    return false
  }
  if (!isObject(parsed)) return false

  const rawType = typeof parsed.type === "string" ? parsed.type : ""
  const canonical = normalizeMcpType(rawType)
  if (canonical !== "http" && canonical !== "sse") return false

  const env = parsed.env
  if (!isObject(env)) return false
  return Object.keys(env).length > 0
}

function parseJsonObject(
  text: string,
  name: string,
  t: McpTranslator
): Record<string, unknown> {
  const trimmed = text.trim()
  if (!trimmed) {
    throw new Error(t("errors.jsonEmpty", { name }))
  }

  let parsed: unknown
  try {
    parsed = JSON.parse(trimmed)
  } catch (err) {
    const message = toLocalizedErrorMessage(err, t)
    throw new Error(t("errors.jsonInvalid", { name, message }))
  }

  if (!isObject(parsed)) {
    throw new Error(t("errors.jsonMustBeObject", { name }))
  }

  return parsed
}

export function McpSettings() {
  const t = useTranslations("McpSettings")
  const mcpT = useMemo(() => t as unknown as McpTranslator, [t])
  const [loading, setLoading] = useState(true)
  const [loadingError, setLoadingError] = useState<string | null>(null)

  const [leftTab, setLeftTab] = useState<LeftTab>("local")
  const [selection, setSelection] = useState<Selection>(null)

  const [installedServers, setInstalledServers] = useState<LocalMcpServer[]>([])
  const [localFilter, setLocalFilter] = useState("")

  const [providers, setProviders] = useState<McpMarketplaceProvider[]>([])
  const [selectedProvider, setSelectedProvider] = useState("")
  const [marketQuery, setMarketQuery] = useState("")
  const marketQueryRef = useRef("")
  const [searching, setSearching] = useState(false)
  const [searchError, setSearchError] = useState<string | null>(null)
  const [searchResults, setSearchResults] = useState<McpMarketplaceItem[]>([])

  const [marketDetail, setMarketDetail] =
    useState<McpMarketplaceServerDetail | null>(null)
  const [marketDetailLoading, setMarketDetailLoading] = useState(false)
  const [marketDetailError, setMarketDetailError] = useState<string | null>(
    null
  )
  const [marketSpecText, setMarketSpecText] = useState("")
  const [marketSpecDirty, setMarketSpecDirty] = useState(false)
  const [selectedInstallOptionId, setSelectedInstallOptionId] = useState("")
  const [installParamDraft, setInstallParamDraft] = useState<
    Record<string, string>
  >({})

  const [localSpecText, setLocalSpecText] = useState("")
  const [localAppsDraft, setLocalAppsDraft] = useState<
    Record<McpAppType, boolean>
  >(appsToDraft([]))

  const [installDialogOpen, setInstallDialogOpen] = useState(false)
  const [installAppsDraft, setInstallAppsDraft] = useState<
    Record<McpAppType, boolean>
  >(appsToDraft(APP_OPTIONS.map((x) => x.value)))

  const [draftServerId, setDraftServerId] = useState("")
  const [draftSpecText, setDraftSpecText] = useState(DEFAULT_DRAFT_SPEC)
  const [draftAppsDraft, setDraftAppsDraft] = useState<
    Record<McpAppType, boolean>
  >(appsToDraft(APP_OPTIONS.map((x) => x.value)))

  const [runningAction, setRunningAction] = useState<string | null>(null)

  const selectedLocal = useMemo(() => {
    if (selection?.kind !== "local") return null
    return installedServers.find((item) => item.id === selection.id) ?? null
  }, [installedServers, selection])

  const selectedMarketItem = useMemo(() => {
    if (selection?.kind !== "market") return null
    return searchResults.find((item) => item.server_id === selection.id) ?? null
  }, [searchResults, selection])

  const selectedInstallOption = useMemo(() => {
    if (!marketDetail) return null
    return (
      marketDetail.install_options.find(
        (item) => item.id === selectedInstallOptionId
      ) ??
      marketDetail.install_options[0] ??
      null
    )
  }, [marketDetail, selectedInstallOptionId])

  const draftEnvOnRemote = useMemo(
    () => detectEnvOnRemote(draftSpecText),
    [draftSpecText]
  )
  const localEnvOnRemote = useMemo(
    () => detectEnvOnRemote(localSpecText),
    [localSpecText]
  )

  const filteredLocalServers = useMemo(() => {
    const q = localFilter.trim().toLowerCase()
    if (!q) return installedServers
    return installedServers.filter((item) => {
      if (item.id.toLowerCase().includes(q)) return true
      const spec = isObject(item.spec) ? item.spec : {}
      return specSummary(spec, mcpT).toLowerCase().includes(q)
    })
  }, [installedServers, localFilter, mcpT])

  const refreshLocalServers = useCallback(async () => {
    const servers = await mcpScanLocal()
    setInstalledServers(servers)
    return servers
  }, [])

  const loadInitial = useCallback(async () => {
    setLoading(true)
    setLoadingError(null)

    try {
      const [servers, marketProviders] = await Promise.all([
        mcpScanLocal(),
        mcpListMarketplaces(),
      ])
      setInstalledServers(servers)
      setProviders(marketProviders)
      setSelectedProvider(
        (current) => current || marketProviders[0]?.id || "official_registry"
      )

      if (servers[0]) {
        setSelection({ kind: "local", id: servers[0].id })
      }
    } catch (err) {
      const message = toLocalizedErrorMessage(err, mcpT)
      setLoadingError(message)
    } finally {
      setLoading(false)
    }
  }, [mcpT])

  useEffect(() => {
    loadInitial().catch((err) => {
      console.error("[Settings] load MCP settings failed:", err)
    })
  }, [loadInitial])

  useEffect(() => {
    if (!selectedLocal) return
    const nextSpec = JSON.stringify(selectedLocal.spec, null, 2)
    setLocalSpecText(nextSpec)
    setLocalAppsDraft(appsToDraft(selectedLocal.apps))
  }, [selectedLocal])

  useEffect(() => {
    if (selection?.kind !== "market" || !selectedMarketItem) {
      setMarketDetail(null)
      setMarketDetailError(null)
      setMarketSpecText("")
      setMarketSpecDirty(false)
      setSelectedInstallOptionId("")
      setInstallParamDraft({})
      return
    }

    setMarketDetailLoading(true)
    setMarketDetailError(null)

    mcpGetMarketplaceServerDetail({
      providerId: selectedMarketItem.provider_id,
      serverId: selectedMarketItem.server_id,
    })
      .then((detail) => {
        setMarketDetail(detail)
        const defaultOption =
          detail.install_options.find(
            (item) => item.id === detail.default_option_id
          ) ??
          detail.install_options[0] ??
          null
        setSelectedInstallOptionId(defaultOption?.id ?? "")
        setInstallParamDraft(defaultParamDraft(defaultOption))
        setMarketSpecText(
          JSON.stringify(defaultOption?.spec ?? detail.spec, null, 2)
        )
        setMarketSpecDirty(false)
      })
      .catch((err) => {
        const message = toLocalizedErrorMessage(err, mcpT)
        setMarketDetailError(message)
        setMarketDetail(null)
        setMarketSpecText("")
        setMarketSpecDirty(false)
        setSelectedInstallOptionId("")
        setInstallParamDraft({})
      })
      .finally(() => {
        setMarketDetailLoading(false)
      })
  }, [selection, selectedMarketItem, mcpT])

  const executeSearch = useCallback(
    async ({
      providerId,
      query,
    }: {
      providerId: string
      query: string | null
    }) => {
      if (!providerId) return

      setSearching(true)
      setSearchError(null)

      try {
        const results = await mcpSearchMarketplace({
          providerId,
          query: query?.trim() || null,
          limit: 30,
        })
        setSearchResults(results)

        if (results[0]) {
          setSelection((current) => {
            if (current?.kind === "market") {
              const hit = results.some((item) => item.server_id === current.id)
              if (hit) return current
            }
            return { kind: "market", id: results[0].server_id }
          })
        }
      } catch (err) {
        const message = toLocalizedErrorMessage(err, mcpT)
        setSearchError(message)
      } finally {
        setSearching(false)
      }
    },
    [mcpT]
  )

  useEffect(() => {
    marketQueryRef.current = marketQuery
  }, [marketQuery])

  useEffect(() => {
    if (leftTab !== "market" || !selectedProvider) return
    executeSearch({
      providerId: selectedProvider,
      query: marketQueryRef.current,
    }).catch((err) => {
      console.error("[Settings] auto search MCP marketplace failed:", err)
    })
  }, [executeSearch, leftTab, selectedProvider])

  const uninstallServer = useCallback(
    async (serverId: string) => {
      const action = `uninstall:${serverId}`
      setRunningAction(action)

      try {
        await mcpRemoveServer(serverId)
        const next = await refreshLocalServers()
        toast.success(t("toasts.uninstalled"))

        setSelection((current) => {
          if (current?.kind !== "local" || current.id !== serverId)
            return current
          if (next[0]) return { kind: "local", id: next[0].id }
          return null
        })
      } catch (err) {
        const message = toLocalizedErrorMessage(err, mcpT)
        toast.error(t("toasts.uninstallFailed", { message }))
      } finally {
        setRunningAction(null)
      }
    },
    [refreshLocalServers, t, mcpT]
  )

  const saveLocalServer = useCallback(async () => {
    if (!selectedLocal) return

    let parsedSpec: Record<string, unknown>
    try {
      parsedSpec = parseJsonObject(
        localSpecText,
        t("jsonNames.localConfig"),
        mcpT
      )
    } catch (err) {
      const message = toLocalizedErrorMessage(err, mcpT)
      toast.error(message)
      return
    }

    // Apps the user can see and toggle in the UI.
    const visibleApps = selectedAppsFromDraft(localAppsDraft)
    // Carry forward assignments for agents no longer offered in the UI (e.g.
    // OpenClaw, which no longer accepts MCP over the ACP wire). We never add
    // these, but must not silently strip a legacy assignment from a server the
    // user is editing — that would destroy existing on-disk config and could
    // wedge an OpenClaw-only server into an unsavable "no apps" state.
    const hiddenLegacyApps = selectedLocal.apps.filter(
      (app) => !APP_OPTIONS.some((option) => option.value === app)
    )
    const apps = normalizeApps([...visibleApps, ...hiddenLegacyApps])
    if (apps.length === 0) {
      toast.error(t("toasts.selectAtLeastOneApp"))
      return
    }

    const action = `save:${selectedLocal.id}`
    setRunningAction(action)

    try {
      await mcpUpsertLocalServer({
        serverId: selectedLocal.id,
        spec: parsedSpec,
        apps,
      })
      const next = await refreshLocalServers()
      toast.success(t("toasts.saveSuccess"))

      const updated = next.find((item) => item.id === selectedLocal.id)
      if (updated) {
        setSelection({ kind: "local", id: updated.id })
        setLocalSpecText(JSON.stringify(updated.spec, null, 2))
        setLocalAppsDraft(appsToDraft(updated.apps))
      }
    } catch (err) {
      const message = toLocalizedErrorMessage(err, mcpT)
      toast.error(t("toasts.saveFailed", { message }))
    } finally {
      setRunningAction(null)
    }
  }, [
    localAppsDraft,
    localSpecText,
    mcpT,
    refreshLocalServers,
    selectedLocal,
    t,
  ])

  const handleCreateDraft = useCallback(() => {
    setLeftTab("local")
    setSelection({ kind: "draft" })
    setDraftServerId("")
    setDraftSpecText(DEFAULT_DRAFT_SPEC)
    setDraftAppsDraft(appsToDraft(APP_OPTIONS.map((item) => item.value)))
  }, [])

  const saveDraft = useCallback(async () => {
    const trimmedId = draftServerId.trim()
    if (!trimmedId) {
      toast.error(t("toasts.serverIdRequired"))
      return
    }

    if (installedServers.some((server) => server.id === trimmedId)) {
      toast.error(t("toasts.serverIdExists", { id: trimmedId }))
      return
    }

    let parsedSpec: Record<string, unknown>
    try {
      parsedSpec = parseJsonObject(
        draftSpecText,
        t("jsonNames.localConfig"),
        mcpT
      )
    } catch (err) {
      const message = toLocalizedErrorMessage(err, mcpT)
      toast.error(message)
      return
    }

    const apps = normalizeApps(selectedAppsFromDraft(draftAppsDraft))
    if (apps.length === 0) {
      toast.error(t("toasts.selectAtLeastOneApp"))
      return
    }

    const action = `create:${trimmedId}`
    setRunningAction(action)

    try {
      await mcpUpsertLocalServer({
        serverId: trimmedId,
        spec: parsedSpec,
        apps,
      })
      const next = await refreshLocalServers()
      toast.success(t("toasts.created"))

      const created = next.find((item) => item.id === trimmedId)
      if (created) {
        setSelection({ kind: "local", id: created.id })
      } else {
        setSelection(null)
      }
    } catch (err) {
      const message = toLocalizedErrorMessage(err, mcpT)
      toast.error(t("toasts.saveFailed", { message }))
    } finally {
      setRunningAction(null)
    }
  }, [
    draftAppsDraft,
    draftServerId,
    draftSpecText,
    installedServers,
    mcpT,
    refreshLocalServers,
    t,
  ])

  const switchInstallOption = useCallback(
    (optionId: string) => {
      if (!marketDetail) return
      const option =
        marketDetail.install_options.find((item) => item.id === optionId) ??
        marketDetail.install_options[0] ??
        null
      setSelectedInstallOptionId(option?.id ?? "")
      setInstallParamDraft(defaultParamDraft(option))
      setMarketSpecText(
        JSON.stringify(option?.spec ?? marketDetail.spec, null, 2)
      )
      setMarketSpecDirty(false)
    },
    [marketDetail]
  )

  const openInstallDialog = useCallback(() => {
    if (!marketDetail) return
    setInstallAppsDraft(appsToDraft(APP_OPTIONS.map((item) => item.value)))
    const option =
      marketDetail.install_options.find(
        (item) => item.id === selectedInstallOptionId
      ) ??
      marketDetail.install_options[0] ??
      null
    setSelectedInstallOptionId(option?.id ?? "")
    setInstallParamDraft(defaultParamDraft(option))
    setInstallDialogOpen(true)
  }, [marketDetail, selectedInstallOptionId])

  const installMarketServer = useCallback(async () => {
    if (!marketDetail) return

    const parsedParams = parseParameterValues(
      selectedInstallOption,
      installParamDraft,
      mcpT
    )
    if (parsedParams.error) {
      toast.error(parsedParams.error)
      return
    }

    let specOverride: Record<string, unknown> | null = null
    const baselineText = JSON.stringify(
      selectedInstallOption?.spec ?? marketDetail.spec,
      null,
      2
    ).trim()
    const currentSpecText = marketSpecText.trim()
    if (marketSpecDirty && currentSpecText !== baselineText) {
      try {
        specOverride = parseJsonObject(
          marketSpecText,
          t("jsonNames.installConfig"),
          mcpT
        )
      } catch (err) {
        const message = toLocalizedErrorMessage(err, mcpT)
        toast.error(message)
        return
      }
    }

    const apps = normalizeApps(selectedAppsFromDraft(installAppsDraft))
    if (apps.length === 0) {
      toast.error(t("toasts.selectAtLeastOneApp"))
      return
    }

    const action = `install:${marketDetail.server_id}`
    setRunningAction(action)

    try {
      await mcpInstallFromMarketplace({
        providerId: marketDetail.provider_id,
        serverId: marketDetail.server_id,
        apps,
        optionId: selectedInstallOption?.id ?? null,
        protocol: selectedInstallOption?.protocol ?? null,
        parameterValues: parsedParams.values,
        specOverride,
      })
      const nextLocal = await refreshLocalServers()
      toast.success(t("toasts.installed", { name: marketDetail.name }))
      setInstallDialogOpen(false)
      setLeftTab("local")

      const installed = nextLocal.find(
        (item) => item.id === marketDetail.server_id
      )
      if (installed) {
        setSelection({ kind: "local", id: installed.id })
      }
    } catch (err) {
      const message = toLocalizedErrorMessage(err, mcpT)
      toast.error(t("toasts.installFailed", { message }))
    } finally {
      setRunningAction(null)
    }
  }, [
    installAppsDraft,
    installParamDraft,
    marketDetail,
    marketSpecDirty,
    marketSpecText,
    mcpT,
    refreshLocalServers,
    selectedInstallOption,
    t,
  ])

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center gap-2 text-sm text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("loading")}
      </div>
    )
  }

  return (
    <>
      <Dialog open={installDialogOpen} onOpenChange={setInstallDialogOpen}>
        <DialogContent>
          <DialogHeader>
            <DialogTitle>{t("installDialog.title")}</DialogTitle>
            <DialogDescription>
              {marketDetail
                ? t("installDialog.descriptionWithName", {
                    name: marketDetail.name,
                  })
                : t("installDialog.description")}
            </DialogDescription>
          </DialogHeader>

          <div className="space-y-4 text-sm">
            <div className="space-y-2">
              <div className="text-xs text-muted-foreground">
                {t("installDialog.protocol")}
              </div>
              <Select
                value={selectedInstallOption?.id ?? ""}
                onValueChange={switchInstallOption}
              >
                <SelectTrigger>
                  <SelectValue
                    placeholder={t("installDialog.selectProtocol")}
                  />
                </SelectTrigger>
                <SelectContent>
                  {(marketDetail?.install_options ?? []).map((option) => (
                    <SelectItem key={option.id} value={option.id}>
                      {protocolBadgeLabel(option.protocol, mcpT)} ·{" "}
                      {option.label}
                    </SelectItem>
                  ))}
                </SelectContent>
              </Select>
            </div>

            {selectedInstallOption?.parameters.length ? (
              <div className="space-y-2">
                <div className="text-xs text-muted-foreground">
                  {t("installDialog.parameters")}
                </div>
                <div className="max-h-56 overflow-auto space-y-2 pr-1">
                  {selectedInstallOption.parameters.map((field) => {
                    const raw = installParamDraft[field.key] ?? ""
                    return (
                      <div key={field.key} className="space-y-1">
                        <div className="text-xs font-medium">
                          {field.label}
                          {field.required ? (
                            <span className="text-red-500 ml-1">*</span>
                          ) : null}
                          {field.location ? (
                            <span className="text-muted-foreground ml-2">
                              {field.location}
                            </span>
                          ) : null}
                        </div>
                        {field.kind === "boolean" ? (
                          <Select
                            value={raw}
                            onValueChange={(value) =>
                              setInstallParamDraft((prev) => ({
                                ...prev,
                                [field.key]: value,
                              }))
                            }
                          >
                            <SelectTrigger>
                              <SelectValue
                                placeholder={t(
                                  "installDialog.booleanPlaceholder"
                                )}
                              />
                            </SelectTrigger>
                            <SelectContent>
                              <SelectItem value="true">true</SelectItem>
                              <SelectItem value="false">false</SelectItem>
                            </SelectContent>
                          </Select>
                        ) : field.enum_values.length > 0 ? (
                          <Select
                            value={raw}
                            onValueChange={(value) =>
                              setInstallParamDraft((prev) => ({
                                ...prev,
                                [field.key]: value,
                              }))
                            }
                          >
                            <SelectTrigger>
                              <SelectValue
                                placeholder={t("installDialog.selectOneValue")}
                              />
                            </SelectTrigger>
                            <SelectContent>
                              {field.enum_values.map((value) => (
                                <SelectItem key={value} value={value}>
                                  {value}
                                </SelectItem>
                              ))}
                            </SelectContent>
                          </Select>
                        ) : field.kind === "json" ? (
                          <Textarea
                            value={raw}
                            onChange={(event) =>
                              setInstallParamDraft((prev) => ({
                                ...prev,
                                [field.key]: event.target.value,
                              }))
                            }
                            className="min-h-20 font-mono text-xs"
                            placeholder={field.placeholder ?? ""}
                          />
                        ) : (
                          <Input
                            type={field.secret ? "password" : "text"}
                            value={raw}
                            onChange={(event) =>
                              setInstallParamDraft((prev) => ({
                                ...prev,
                                [field.key]: event.target.value,
                              }))
                            }
                            placeholder={field.placeholder ?? ""}
                          />
                        )}
                        {field.description ? (
                          <div className="text-[11px] text-muted-foreground leading-5">
                            {field.description}
                          </div>
                        ) : null}
                      </div>
                    )
                  })}
                </div>
              </div>
            ) : null}

            <div className="space-y-2">
              <div className="text-xs text-muted-foreground">
                {t("installDialog.targetApps")}
              </div>
              {APP_OPTIONS.map((app) => (
                <label
                  key={app.value}
                  className="inline-flex w-full items-center gap-2 rounded-md border px-2 py-1.5"
                >
                  <input
                    type="checkbox"
                    checked={installAppsDraft[app.value]}
                    onChange={(event) => {
                      setInstallAppsDraft((prev) => ({
                        ...prev,
                        [app.value]: event.target.checked,
                      }))
                    }}
                  />
                  <span>{app.label}</span>
                </label>
              ))}
            </div>
          </div>

          <DialogFooter>
            <Button
              variant="outline"
              onClick={() => setInstallDialogOpen(false)}
              disabled={Boolean(runningAction?.startsWith("install:"))}
            >
              {t("actions.cancel")}
            </Button>
            <Button
              onClick={() => {
                installMarketServer().catch((err) => {
                  console.error("[Settings] install MCP failed:", err)
                })
              }}
              disabled={Boolean(runningAction?.startsWith("install:"))}
            >
              {runningAction?.startsWith("install:") ? (
                <>
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                  {t("actions.installing")}
                </>
              ) : (
                t("actions.confirmInstall")
              )}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>

      <div className="h-full min-h-0 grid grid-cols-1 gap-4 p-3 md:p-4 lg:grid-cols-[360px_1fr]">
        <section className="min-h-0 rounded-xl border bg-card p-3">
          <Tabs
            value={leftTab}
            onValueChange={(value) => setLeftTab(value as LeftTab)}
            className="h-full"
          >
            <TabsList className="w-full">
              <TabsTrigger value="local" className="flex-1">
                {t("tabs.local")}
              </TabsTrigger>
              <TabsTrigger value="market" className="flex-1">
                {t("tabs.market")}
              </TabsTrigger>
            </TabsList>

            <TabsContent
              value="local"
              className="h-full min-h-0 pt-2 flex flex-col"
            >
              <div className="pb-2">
                <Input
                  value={localFilter}
                  onChange={(event) => setLocalFilter(event.target.value)}
                  placeholder={t("local.filterPlaceholder")}
                />
              </div>

              {loadingError ? (
                <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
                  {t("local.loadFailed", { message: loadingError })}
                </div>
              ) : null}

              <div className="flex-1 min-h-0 overflow-auto space-y-1">
                {filteredLocalServers.length === 0 ? (
                  <div className="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
                    {t("local.empty")}
                  </div>
                ) : (
                  filteredLocalServers.map((server) => {
                    const active =
                      selection?.kind === "local" && selection.id === server.id
                    const spec = isObject(server.spec) ? server.spec : {}
                    return (
                      <ContextMenu key={server.id}>
                        <ContextMenuTrigger asChild>
                          <button
                            className={cn(
                              "w-full rounded-md border p-2 text-left transition-colors",
                              active
                                ? "border-primary bg-primary/5"
                                : "hover:bg-muted/60"
                            )}
                            onClick={() => {
                              setSelection({ kind: "local", id: server.id })
                            }}
                          >
                            <div className="text-sm font-medium break-all">
                              {server.id}
                            </div>
                            <div className="text-xs text-muted-foreground line-clamp-2 break-all">
                              {specSummary(spec, mcpT)}
                            </div>
                          </button>
                        </ContextMenuTrigger>
                        <ContextMenuContent>
                          <ContextMenuItem
                            variant="destructive"
                            onClick={() => {
                              uninstallServer(server.id).catch((err) => {
                                console.error(
                                  "[Settings] uninstall MCP failed:",
                                  err
                                )
                              })
                            }}
                          >
                            {t("actions.uninstall")}
                          </ContextMenuItem>
                        </ContextMenuContent>
                      </ContextMenu>
                    )
                  })
                )}
              </div>

              <div className="border-t pt-2 mt-2 flex items-center gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  className="flex-1"
                  onClick={() => {
                    refreshLocalServers().catch((err) => {
                      console.error("[Settings] refresh local MCP failed:", err)
                    })
                  }}
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                  {t("actions.refresh")}
                </Button>
                <Button
                  size="sm"
                  className="flex-1"
                  onClick={handleCreateDraft}
                >
                  <Plus className="h-3.5 w-3.5" />
                  {t("actions.newMcp")}
                </Button>
              </div>
            </TabsContent>

            <TabsContent value="market" className="h-full min-h-0 pt-2">
              <div className="space-y-2 pb-2">
                <Select
                  value={selectedProvider}
                  onValueChange={setSelectedProvider}
                >
                  <SelectTrigger>
                    <SelectValue placeholder={t("market.selectMarketplace")} />
                  </SelectTrigger>
                  <SelectContent>
                    {providers.map((provider) => (
                      <SelectItem key={provider.id} value={provider.id}>
                        {provider.name}
                      </SelectItem>
                    ))}
                  </SelectContent>
                </Select>

                <div className="flex gap-2">
                  <Input
                    value={marketQuery}
                    onChange={(event) => setMarketQuery(event.target.value)}
                    placeholder={t("market.searchPlaceholder")}
                    onKeyDown={(event) => {
                      if (event.key !== "Enter") return
                      executeSearch({
                        providerId: selectedProvider,
                        query: marketQuery,
                      }).catch((err) => {
                        console.error(
                          "[Settings] search MCP marketplace failed:",
                          err
                        )
                      })
                    }}
                  />
                  <Button
                    onClick={() => {
                      executeSearch({
                        providerId: selectedProvider,
                        query: marketQuery,
                      }).catch((err) => {
                        console.error(
                          "[Settings] search MCP marketplace failed:",
                          err
                        )
                      })
                    }}
                    disabled={searching || !selectedProvider}
                  >
                    {searching ? (
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    ) : (
                      <Search className="h-3.5 w-3.5" />
                    )}
                  </Button>
                </div>
              </div>

              {searchError ? (
                <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
                  {t("market.searchFailed", { message: searchError })}
                </div>
              ) : null}

              <div className="h-[calc(100%-106px)] overflow-auto space-y-1">
                {searching ? (
                  <div className="h-full min-h-24 rounded-md border border-dashed flex items-center justify-center gap-2 text-xs text-muted-foreground">
                    <Loader2 className="h-3.5 w-3.5 animate-spin" />
                    {t("market.loadingList")}
                  </div>
                ) : searchResults.length === 0 ? (
                  <div className="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
                    {t("market.empty")}
                  </div>
                ) : (
                  searchResults.map((item) => {
                    const active =
                      selection?.kind === "market" &&
                      selection.id === item.server_id
                    return (
                      <ContextMenu
                        key={`${item.provider_id}:${item.server_id}`}
                      >
                        <ContextMenuTrigger asChild>
                          <button
                            className={cn(
                              "w-full rounded-md border p-2 text-left transition-colors",
                              active
                                ? "border-primary bg-primary/5"
                                : "hover:bg-muted/60"
                            )}
                            onClick={() => {
                              setSelection({
                                kind: "market",
                                id: item.server_id,
                              })
                            }}
                          >
                            <div className="flex items-start gap-2">
                              <div className="mt-0.5 h-7 w-7 overflow-hidden rounded-md border bg-muted/40 shrink-0">
                                {item.icon_url ? (
                                  // eslint-disable-next-line @next/next/no-img-element
                                  <img
                                    src={item.icon_url}
                                    alt={item.name}
                                    className="h-full w-full object-cover"
                                  />
                                ) : (
                                  <div className="h-full w-full flex items-center justify-center text-[10px] text-muted-foreground">
                                    MCP
                                  </div>
                                )}
                              </div>
                              <div className="min-w-0 flex-1">
                                <div className="text-sm font-medium break-all">
                                  {item.name}
                                </div>
                                <div className="text-xs text-muted-foreground break-all">
                                  {item.server_id}
                                </div>
                              </div>
                            </div>

                            <div className="mt-2 flex flex-wrap gap-1">
                              {item.protocols.map((protocol) => (
                                <Badge
                                  key={`${item.server_id}-${protocol}`}
                                  variant="secondary"
                                  className="text-[10px]"
                                >
                                  {protocolBadgeLabel(protocol, mcpT)}
                                </Badge>
                              ))}
                              {item.latest_version ? (
                                <Badge
                                  variant="outline"
                                  className="text-[10px]"
                                >
                                  v{item.latest_version}
                                </Badge>
                              ) : null}
                              {item.verified ? (
                                <Badge className="text-[10px]">
                                  {t("badges.verified")}
                                </Badge>
                              ) : null}
                              {typeof item.downloads === "number" ? (
                                <Badge
                                  variant="outline"
                                  className="text-[10px]"
                                >
                                  {t("badges.uses", { count: item.downloads })}
                                </Badge>
                              ) : null}
                            </div>
                          </button>
                        </ContextMenuTrigger>
                        <ContextMenuContent>
                          <ContextMenuItem
                            onClick={() => {
                              setSelection({
                                kind: "market",
                                id: item.server_id,
                              })
                            }}
                          >
                            {t("actions.viewDetails")}
                          </ContextMenuItem>
                        </ContextMenuContent>
                      </ContextMenu>
                    )
                  })
                )}
              </div>
            </TabsContent>
          </Tabs>
        </section>

        <section className="min-h-0 rounded-xl border bg-card p-4 overflow-auto">
          {selection?.kind === "draft" ? (
            <div className="space-y-4">
              <div>
                <h2 className="text-base font-semibold">
                  {t("local.draftTitle")}
                </h2>
                <p className="text-xs text-muted-foreground mt-1">
                  {t("local.draftDescription")}
                </p>
              </div>

              <div className="space-y-2">
                <div className="text-xs text-muted-foreground">
                  {t("local.serverIdLabel")}
                </div>
                <Input
                  value={draftServerId}
                  onChange={(event) => setDraftServerId(event.target.value)}
                  placeholder={t("local.serverIdPlaceholder")}
                />
              </div>

              <div className="space-y-2">
                <div className="text-xs text-muted-foreground">
                  {t("local.enabledApps")}
                </div>
                <div className="flex flex-wrap gap-2">
                  {APP_OPTIONS.map((app) => (
                    <label
                      key={app.value}
                      className="inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs"
                    >
                      <input
                        type="checkbox"
                        checked={draftAppsDraft[app.value]}
                        onChange={(event) => {
                          setDraftAppsDraft((prev) => ({
                            ...prev,
                            [app.value]: event.target.checked,
                          }))
                        }}
                      />
                      {app.label}
                    </label>
                  ))}
                </div>
              </div>

              <div className="space-y-2">
                <div className="text-xs text-muted-foreground">
                  {t("local.configJson")}
                </div>
                <p className="text-xs text-muted-foreground">
                  {t("local.typeHint")}
                </p>
                <Textarea
                  value={draftSpecText}
                  onChange={(event) => setDraftSpecText(event.target.value)}
                  className="min-h-[360px] font-mono text-xs"
                />
              </div>

              {draftEnvOnRemote ? (
                <div className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-600 dark:text-amber-400">
                  {t("local.envOnRemoteWarning")}
                </div>
              ) : null}

              <div className="flex justify-end gap-2">
                <Button
                  variant="outline"
                  onClick={() => setSelection(null)}
                  disabled={Boolean(runningAction?.startsWith("create:"))}
                >
                  {t("actions.cancel")}
                </Button>
                <Button
                  onClick={() => {
                    saveDraft().catch((err) => {
                      console.error("[Settings] create local MCP failed:", err)
                    })
                  }}
                  disabled={Boolean(runningAction?.startsWith("create:"))}
                >
                  {runningAction?.startsWith("create:") ? (
                    <>
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t("actions.creating")}
                    </>
                  ) : (
                    t("actions.create")
                  )}
                </Button>
              </div>
            </div>
          ) : null}

          {selection?.kind === "local" && selectedLocal ? (
            <div className="space-y-4">
              <div className="flex items-start justify-between gap-3">
                <div>
                  <h2 className="text-base font-semibold break-all">
                    {selectedLocal.id}
                  </h2>
                  <p className="text-xs text-muted-foreground mt-1">
                    {t("local.description")}
                  </p>
                </div>
                <Button
                  variant="destructive"
                  onClick={() => {
                    uninstallServer(selectedLocal.id).catch((err) => {
                      console.error("[Settings] uninstall MCP failed:", err)
                    })
                  }}
                  disabled={runningAction === `uninstall:${selectedLocal.id}`}
                >
                  {runningAction === `uninstall:${selectedLocal.id}` ? (
                    <>
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t("actions.uninstalling")}
                    </>
                  ) : (
                    t("actions.uninstall")
                  )}
                </Button>
              </div>

              <div className="space-y-2">
                <div className="text-xs text-muted-foreground">
                  {t("local.enabledApps")}
                </div>
                <div className="flex flex-wrap gap-2">
                  {APP_OPTIONS.map((app) => (
                    <label
                      key={app.value}
                      className="inline-flex items-center gap-1.5 rounded-md border px-2 py-1 text-xs"
                    >
                      <input
                        type="checkbox"
                        checked={localAppsDraft[app.value]}
                        onChange={(event) => {
                          setLocalAppsDraft((prev) => ({
                            ...prev,
                            [app.value]: event.target.checked,
                          }))
                        }}
                      />
                      {app.label}
                    </label>
                  ))}
                </div>
              </div>

              <div className="space-y-2">
                <div className="text-xs text-muted-foreground">
                  {t("local.configJson")}
                </div>
                <p className="text-xs text-muted-foreground">
                  {t("local.typeHint")}
                </p>
                <Textarea
                  value={localSpecText}
                  onChange={(event) => setLocalSpecText(event.target.value)}
                  className="min-h-[360px] font-mono text-xs"
                />
              </div>

              {localEnvOnRemote ? (
                <div className="rounded-md border border-amber-500/30 bg-amber-500/10 px-3 py-2 text-xs text-amber-600 dark:text-amber-400">
                  {t("local.envOnRemoteWarning")}
                </div>
              ) : null}

              <div className="flex justify-end">
                <Button
                  onClick={() => {
                    saveLocalServer().catch((err) => {
                      console.error("[Settings] save local MCP failed:", err)
                    })
                  }}
                  disabled={runningAction === `save:${selectedLocal.id}`}
                >
                  {runningAction === `save:${selectedLocal.id}` ? (
                    <>
                      <Loader2 className="h-3.5 w-3.5 animate-spin" />
                      {t("actions.saving")}
                    </>
                  ) : (
                    t("actions.save")
                  )}
                </Button>
              </div>
            </div>
          ) : null}

          {selection?.kind === "market" ? (
            <div className="space-y-4">
              {marketDetailLoading ? (
                <div className="h-40 flex items-center justify-center gap-2 text-sm text-muted-foreground">
                  <Loader2 className="h-4 w-4 animate-spin" />
                  {t("market.loadingDetail")}
                </div>
              ) : marketDetailError ? (
                <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
                  {t("market.detailLoadFailed", { message: marketDetailError })}
                </div>
              ) : marketDetail ? (
                <>
                  <div className="flex items-start justify-between gap-3">
                    <div className="flex items-start gap-3 min-w-0">
                      <div className="h-12 w-12 overflow-hidden rounded-lg border bg-muted/40 shrink-0">
                        {marketDetail.icon_url ? (
                          // eslint-disable-next-line @next/next/no-img-element
                          <img
                            src={marketDetail.icon_url}
                            alt={marketDetail.name}
                            className="h-full w-full object-cover"
                          />
                        ) : (
                          <div className="h-full w-full flex items-center justify-center text-xs text-muted-foreground">
                            MCP
                          </div>
                        )}
                      </div>
                      <div className="min-w-0">
                        <h2 className="text-base font-semibold break-all">
                          {marketDetail.name}
                        </h2>
                        <p className="text-xs text-muted-foreground break-all mt-1">
                          {marketDetail.server_id}
                        </p>
                      </div>
                    </div>
                    <Button onClick={openInstallDialog}>
                      {t("actions.install")}
                    </Button>
                  </div>

                  <div className="flex flex-wrap gap-1.5">
                    {marketDetail.verified ? (
                      <Badge>{t("badges.verified")}</Badge>
                    ) : null}
                    {marketDetail.remote ? (
                      <Badge variant="secondary">{t("badges.remote")}</Badge>
                    ) : null}
                    {marketDetail.homepage ? (
                      <Badge variant="outline">{t("badges.hasHomepage")}</Badge>
                    ) : null}
                    {marketDetail.protocols.map((protocol) => (
                      <Badge key={`detail-${protocol}`} variant="secondary">
                        {protocolBadgeLabel(protocol, mcpT)}
                      </Badge>
                    ))}
                    {marketDetail.latest_version ? (
                      <Badge variant="outline">
                        v{marketDetail.latest_version}
                      </Badge>
                    ) : null}
                    {typeof marketDetail.downloads === "number" ? (
                      <Badge variant="outline">
                        {t("badges.uses", { count: marketDetail.downloads })}
                      </Badge>
                    ) : null}
                  </div>

                  <p className="text-sm text-muted-foreground leading-6">
                    {marketDetail.description}
                  </p>

                  {marketDetail.homepage ? (
                    <a
                      href={marketDetail.homepage}
                      target="_blank"
                      rel="noreferrer"
                      className="text-xs text-primary underline break-all"
                    >
                      {marketDetail.homepage}
                    </a>
                  ) : null}

                  <div className="grid gap-2 text-xs text-muted-foreground sm:grid-cols-2">
                    {marketDetail.owner ? (
                      <div className="inline-flex items-center gap-1.5">
                        <ShieldCheck className="h-3.5 w-3.5" />
                        {t("market.owner", { owner: marketDetail.owner })}
                      </div>
                    ) : null}
                    {marketDetail.namespace ? (
                      <div className="inline-flex items-center gap-1.5">
                        <TerminalSquare className="h-3.5 w-3.5" />
                        {t("market.namespace", {
                          namespace: marketDetail.namespace,
                        })}
                      </div>
                    ) : null}
                    {marketDetail.is_deployed != null ? (
                      <div className="inline-flex items-center gap-1.5">
                        <Globe className="h-3.5 w-3.5" />
                        {marketDetail.is_deployed
                          ? t("badges.deployed")
                          : t("badges.notDeployed")}
                      </div>
                    ) : null}
                  </div>

                  <div className="space-y-2">
                    <div className="text-xs text-muted-foreground">
                      {t("market.defaultInstallProtocol")}
                    </div>
                    <Select
                      value={selectedInstallOption?.id ?? ""}
                      onValueChange={switchInstallOption}
                    >
                      <SelectTrigger>
                        <SelectValue
                          placeholder={t("installDialog.selectProtocol")}
                        />
                      </SelectTrigger>
                      <SelectContent>
                        {marketDetail.install_options.map((option) => (
                          <SelectItem key={option.id} value={option.id}>
                            {protocolBadgeLabel(option.protocol, mcpT)} ·{" "}
                            {option.label}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                    <div className="text-[11px] text-muted-foreground">
                      {t("market.currentOptionParameterCount", {
                        count: selectedInstallOption?.parameters.length ?? 0,
                      })}
                    </div>
                  </div>

                  <div className="space-y-2">
                    <div className="text-xs text-muted-foreground">
                      {t("market.installConfigDescription")}
                    </div>
                    <Textarea
                      value={marketSpecText}
                      onChange={(event) => {
                        setMarketSpecText(event.target.value)
                        setMarketSpecDirty(true)
                      }}
                      className="min-h-[360px] font-mono text-xs"
                    />
                  </div>
                </>
              ) : (
                <div className="rounded-md border border-dashed p-3 text-xs text-muted-foreground">
                  {t("market.selectLeftToView")}
                </div>
              )}
            </div>
          ) : null}

          {!selection ? (
            <div className="h-full flex items-center justify-center text-sm text-muted-foreground">
              {t("selectLeftMcp")}
            </div>
          ) : null}
        </section>
      </div>
    </>
  )
}
