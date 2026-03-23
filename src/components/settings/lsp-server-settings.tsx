"use client"

import { useCallback, useEffect, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import {
  AlertCircle,
  ChevronDown,
  ChevronRight,
  Download,
  Loader2,
  RefreshCw,
  Trash2,
} from "lucide-react"
import { openUrl } from "@tauri-apps/plugin-opener"
import { toast } from "sonner"
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
import { Switch } from "@/components/ui/switch"
import { cn } from "@/lib/utils"
import {
  lspListServers,
  lspPreflight,
  lspInstallServer,
  lspUpgradeServer,
  lspUninstallServer,
  lspUpdateServerPreferences,
  lspClearBinaryCache,
  lspDetectServerVersion,
} from "@/lib/tauri"
import type {
  LspServerInfo,
  LspPreflightResult,
  CheckStatus,
  FixAction,
} from "@/lib/types"

const DISTRIBUTION_LABELS: Record<string, string> = {
  npm: "npm",
  binary: "Binary",
  cargo_install: "Cargo",
  pip_install: "pip",
}

function summarizeCheckStatus(
  state: ServerCheckState | undefined,
  isInstalled: boolean
): CheckStatus | "unchecked" {
  if (!state?.result) return "unchecked"
  if (state.error) return "fail"
  const { checks } = state.result
  if (checks.some((c) => c.status === "fail")) return "fail"
  if (checks.some((c) => c.status === "warn")) return "warn"
  // Environment passed but server not installed — treat as warn
  if (!isInstalled) return "warn"
  return "pass"
}

interface ServerCheckState {
  result?: LspPreflightResult
  error?: string
}

type RunningAction = "install" | "upgrade" | "uninstall" | "detect_version"

export function LspServerSettings() {
  const t = useTranslations("LspSettings")
  const [servers, setServers] = useState<LspServerInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [loadingError, setLoadingError] = useState<string | null>(null)
  const [checkState, setCheckState] = useState<
    Record<string, ServerCheckState>
  >({})
  const [checking, setChecking] = useState<Record<string, boolean>>({})
  const [busyAction, setBusyAction] = useState<Record<string, boolean>>({})
  const [runningAction, setRunningAction] = useState<
    Record<string, RunningAction>
  >({})
  const [uninstallConfirm, setUninstallConfirm] =
    useState<LspServerInfo | null>(null)
  const [expandedChecks, setExpandedChecks] = useState<Record<string, boolean>>(
    {}
  )
  const [selectedId, setSelectedId] = useState<string | null>(null)

  const sortedServers = useMemo(
    () =>
      [...servers].sort(
        (a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name)
      ),
    [servers]
  )

  const serverIdsKey = useMemo(
    () =>
      servers
        .map((s) => s.id)
        .sort()
        .join(","),
    [servers]
  )

  const refreshServers = useCallback(async () => {
    setLoading(true)
    setLoadingError(null)
    try {
      const next = await lspListServers()
      setServers(next)
    } catch (err) {
      const message = err instanceof Error ? err.message : String(err)
      setLoadingError(message)
    } finally {
      setLoading(false)
    }
  }, [])

  const runPreflight = useCallback(async (serverId: string) => {
    setChecking((prev) => ({ ...prev, [serverId]: true }))
    try {
      const [resultState, versionState] = await Promise.allSettled([
        lspPreflight(serverId),
        lspDetectServerVersion(serverId),
      ])

      const result =
        resultState.status === "fulfilled" ? resultState.value : undefined
      const detectedVersion =
        versionState.status === "fulfilled" ? versionState.value : undefined

      if (detectedVersion !== undefined) {
        setServers((prev) =>
          prev.map((s) =>
            s.id === serverId ? { ...s, installed_version: detectedVersion } : s
          )
        )
      }

      setCheckState((prev) => ({
        ...prev,
        [serverId]: result
          ? { result }
          : {
              error:
                resultState.status === "rejected"
                  ? String(resultState.reason)
                  : "Preflight check failed",
            },
      }))
    } catch (err) {
      setCheckState((prev) => ({
        ...prev,
        [serverId]: {
          error: err instanceof Error ? err.message : String(err),
        },
      }))
    } finally {
      setChecking((prev) => ({ ...prev, [serverId]: false }))
    }
  }, [])

  const runAllPreflight = useCallback(
    async (serverIds: string[]) => {
      if (serverIds.length === 0) return
      setChecking((prev) => {
        const next = { ...prev }
        for (const id of serverIds) {
          next[id] = true
        }
        return next
      })
      await Promise.all(serverIds.map((id) => runPreflight(id)))
    },
    [runPreflight]
  )

  useEffect(() => {
    refreshServers()
  }, [refreshServers])

  // Auto-run preflight for all servers after loading
  useEffect(() => {
    if (loading || !serverIdsKey) return
    const ids = serverIdsKey.split(",")
    runAllPreflight(ids).catch((err) => {
      console.error("[LSP] run all preflight failed:", err)
    })
  }, [serverIdsKey, loading, runAllPreflight])

  const handleRefreshAll = useCallback(async () => {
    await refreshServers()
  }, [refreshServers])

  const handleInstall = useCallback(
    async (server: LspServerInfo) => {
      setBusyAction((prev) => ({ ...prev, [server.id]: true }))
      setRunningAction((prev) => ({ ...prev, [server.id]: "install" }))
      try {
        await lspInstallServer(server.id)
        toast.success(t("installSuccess", { name: server.name }))
        await refreshServers()
        runPreflight(server.id)
      } catch (err) {
        toast.error(
          t("installError", {
            name: server.name,
            error: err instanceof Error ? err.message : String(err),
          })
        )
      } finally {
        setBusyAction((prev) => ({ ...prev, [server.id]: false }))
        setRunningAction((prev) => {
          const next = { ...prev }
          delete next[server.id]
          return next
        })
      }
    },
    [refreshServers, runPreflight, t]
  )

  const handleUpgrade = useCallback(
    async (server: LspServerInfo) => {
      setBusyAction((prev) => ({ ...prev, [server.id]: true }))
      setRunningAction((prev) => ({ ...prev, [server.id]: "upgrade" }))
      try {
        await lspUpgradeServer(server.id)
        toast.success(t("upgradeSuccess", { name: server.name }))
        await refreshServers()
        runPreflight(server.id)
      } catch (err) {
        toast.error(
          t("upgradeError", {
            name: server.name,
            error: err instanceof Error ? err.message : String(err),
          })
        )
      } finally {
        setBusyAction((prev) => ({ ...prev, [server.id]: false }))
        setRunningAction((prev) => {
          const next = { ...prev }
          delete next[server.id]
          return next
        })
      }
    },
    [refreshServers, runPreflight, t]
  )

  const handleUninstall = useCallback(
    async (server: LspServerInfo) => {
      setUninstallConfirm(null)
      setBusyAction((prev) => ({ ...prev, [server.id]: true }))
      setRunningAction((prev) => ({ ...prev, [server.id]: "uninstall" }))
      try {
        await lspUninstallServer(server.id)
        if (server.distribution_type === "binary") {
          await lspClearBinaryCache(server.id)
        }
        toast.success(t("uninstallSuccess", { name: server.name }))
        // Also disable if was enabled
        if (server.enabled) {
          await lspUpdateServerPreferences(server.id, false, server.config_json)
        }
        await refreshServers()
        runPreflight(server.id)
      } catch (err) {
        toast.error(
          t("uninstallError", {
            name: server.name,
            error: err instanceof Error ? err.message : String(err),
          })
        )
      } finally {
        setBusyAction((prev) => ({ ...prev, [server.id]: false }))
        setRunningAction((prev) => {
          const next = { ...prev }
          delete next[server.id]
          return next
        })
      }
    },
    [refreshServers, runPreflight, t]
  )

  const handleToggleEnabled = useCallback(
    async (server: LspServerInfo, enabled: boolean) => {
      try {
        await lspUpdateServerPreferences(server.id, enabled, server.config_json)
        setServers((prev) =>
          prev.map((s) => (s.id === server.id ? { ...s, enabled } : s))
        )
      } catch (err) {
        toast.error(String(err))
      }
    },
    []
  )

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-muted-foreground">
        <Loader2 className="mr-2 h-4 w-4 animate-spin" />
        {t("loading")}
      </div>
    )
  }

  if (loadingError) {
    return (
      <div className="h-full flex flex-col items-center justify-center gap-2 text-sm text-muted-foreground">
        <AlertCircle className="h-5 w-5 text-destructive" />
        <p>{loadingError}</p>
        <Button variant="outline" size="sm" onClick={refreshServers}>
          <RefreshCw className="mr-1.5 h-3.5 w-3.5" />
          {t("retry")}
        </Button>
      </div>
    )
  }

  return (
    <div className="flex h-full">
      <div className="w-full flex flex-col">
        <div className="flex items-center justify-between gap-3 pb-4">
          <div>
            <h2 className="text-base font-semibold">{t("title")}</h2>
            <p className="text-xs text-muted-foreground mt-1">
              {t("description")}
            </p>
          </div>
          <Button variant="ghost" size="icon-sm" onClick={handleRefreshAll}>
            <RefreshCw className="h-3.5 w-3.5" />
          </Button>
        </div>

        <div className="flex-1 overflow-y-auto pb-4">
          <div className="grid gap-3">
            {sortedServers.map((server) => {
              const isChecking = !!checking[server.id]
              const state = checkState[server.id]
              const summary = summarizeCheckStatus(
                state,
                !!server.installed_version
              )
              const displayStatus: CheckStatus | "unchecked" | "checking" =
                isChecking ? "checking" : summary

              return (
                <LspServerCard
                  key={server.id}
                  server={server}
                  selected={selectedId === server.id}
                  onSelect={() =>
                    setSelectedId((prev) =>
                      prev === server.id ? null : server.id
                    )
                  }
                  checking={isChecking}
                  checkState={state}
                  displayStatus={displayStatus}
                  busy={!!busyAction[server.id]}
                  runningAction={runningAction[server.id]}
                  expandedChecks={expandedChecks}
                  onToggleCheck={(checkKey) =>
                    setExpandedChecks((prev) => ({
                      ...prev,
                      [checkKey]: !prev[checkKey],
                    }))
                  }
                  onRunPreflight={() => runPreflight(server.id)}
                  onInstall={() => handleInstall(server)}
                  onUpgrade={() => handleUpgrade(server)}
                  onUninstall={() => setUninstallConfirm(server)}
                  onToggleEnabled={(enabled) =>
                    handleToggleEnabled(server, enabled)
                  }
                />
              )
            })}
          </div>
        </div>
      </div>

      {/* Uninstall confirm dialog */}
      <AlertDialog
        open={!!uninstallConfirm}
        onOpenChange={(open) => {
          if (!open) setUninstallConfirm(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>
              {t("uninstallConfirmTitle", {
                name: uninstallConfirm?.name ?? "",
              })}
            </AlertDialogTitle>
            <AlertDialogDescription>
              {t("uninstallConfirmDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <Button
              variant="destructive"
              onClick={() =>
                uninstallConfirm && handleUninstall(uninstallConfirm)
              }
            >
              {t("uninstall")}
            </Button>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}

// ─── Server Card ──────────────────────────────────────────────────────────

interface LspServerCardProps {
  server: LspServerInfo
  selected: boolean
  onSelect: () => void
  checking: boolean
  checkState?: ServerCheckState
  displayStatus: CheckStatus | "unchecked" | "checking"
  busy: boolean
  runningAction?: RunningAction
  expandedChecks: Record<string, boolean>
  onToggleCheck: (checkKey: string) => void
  onRunPreflight: () => void
  onInstall: () => void
  onUpgrade: () => void
  onUninstall: () => void
  onToggleEnabled: (enabled: boolean) => void
}

function LspServerCard({
  server,
  selected,
  onSelect,
  checking,
  checkState,
  displayStatus,
  busy,
  runningAction,
  expandedChecks,
  onToggleCheck,
  onRunPreflight,
  onInstall,
  onUpgrade,
  onUninstall,
  onToggleEnabled,
}: LspServerCardProps) {
  const t = useTranslations("LspSettings")

  const isInstalled = !!server.installed_version
  const preflightPassed = checkState?.result?.passed === true

  // Can only enable when installed AND preflight passed
  const canEnable = isInstalled && preflightPassed

  const ExpandIcon = selected ? ChevronDown : ChevronRight

  // Status badge styling (like agents)
  const statusLabel =
    displayStatus === "unchecked"
      ? t("statusUnchecked")
      : displayStatus === "checking"
        ? "Checking"
        : displayStatus.toUpperCase()

  const statusToneClass = !server.enabled
    ? "border-muted-foreground/30 bg-muted/30 text-muted-foreground"
    : displayStatus === "pass"
      ? "border-green-500/40 bg-green-500/10 text-green-600 dark:text-green-400"
      : displayStatus === "fail"
        ? "border-red-500/40 bg-red-500/10 text-red-500"
        : displayStatus === "warn"
          ? "border-yellow-500/40 bg-yellow-500/10 text-yellow-600 dark:text-yellow-400"
          : displayStatus === "checking"
            ? "border-blue-500/40 bg-blue-500/10 text-blue-600 dark:text-blue-400"
            : "border-muted-foreground/30 bg-muted/30 text-muted-foreground"

  return (
    <div className="rounded-lg border bg-card">
      {/* Header */}
      <div className="flex items-center gap-2 p-3">
        <button
          className="flex-1 flex items-center gap-2 text-left min-w-0"
          onClick={onSelect}
        >
          <ExpandIcon className="h-4 w-4 shrink-0 text-muted-foreground" />
          <span className="text-sm text-muted-foreground truncate">
            {t("language")}: {server.language}
          </span>
          <Badge variant="outline" className="text-[10px] px-1.5 py-0 shrink-0">
            {DISTRIBUTION_LABELS[server.distribution_type] ??
              server.distribution_type}
          </Badge>
          <span className="text-xs text-muted-foreground truncate">
            {server.description}
          </span>
        </button>

        {/* Status badge with inline refresh */}
        <Badge
          variant="outline"
          className={cn(
            "h-6 px-2 inline-flex items-center gap-1 text-xs leading-none shrink-0",
            statusToneClass
          )}
        >
          <span>{statusLabel}</span>
          {displayStatus === "checking" && (
            <Loader2 className="h-3.5 w-3.5 animate-spin shrink-0" />
          )}
          {!checking && (
            <button
              type="button"
              className="inline-flex h-4 w-4 items-center justify-center rounded hover:bg-black/10 dark:hover:bg-white/10"
              title={t("preflight")}
              onClick={(e) => {
                e.stopPropagation()
                onRunPreflight()
              }}
            >
              <RefreshCw className="h-3 w-3 shrink-0" />
            </button>
          )}
        </Badge>

        {/* Enable/disable switch */}
        <Switch
          checked={server.enabled}
          onCheckedChange={(checked) => onToggleEnabled(checked)}
          disabled={!server.enabled && !canEnable}
          className="shrink-0"
        />
      </div>

      {/* Expanded detail */}
      <Collapsible open={selected}>
        <CollapsibleContent>
          <div className="px-3 pb-3 space-y-3 pt-3">
            <div className="text-sm font-medium">{server.name}</div>
            <p className="text-xs text-muted-foreground">
              {server.description}
            </p>

            {/* Check items (version + preflight) */}
            <CheckItemList
              server={server}
              checkState={checkState}
              expandedChecks={expandedChecks}
              onToggleCheck={onToggleCheck}
              busy={busy}
              runningAction={runningAction}
              preflightPassed={preflightPassed}
              onInstall={onInstall}
              onUpgrade={onUpgrade}
              onUninstall={onUninstall}
            />
          </div>
        </CollapsibleContent>
      </Collapsible>
    </div>
  )
}

// ─── Check Item List (version + preflight, agents-style) ─────────────────

function statusTone(status: CheckStatus): string {
  if (status === "pass") return "text-green-500"
  if (status === "warn") return "text-yellow-500"
  return "text-red-500"
}

interface UiCheckItem {
  check_id: string
  label: string
  status: CheckStatus
  message: string
  fixes: FixAction[]
}

function buildVersionCheck(
  server: LspServerInfo,
  t: (key: string) => string
): UiCheckItem {
  const remoteVersion = server.registry_version ?? "-"
  const localVersion = server.installed_version ?? "-"
  const versionText = `${t("remoteVersion")}: ${remoteVersion} · ${t("localVersion")}: ${localVersion}`

  if (!server.installed_version) {
    return {
      check_id: "version_status",
      label: t("versionStatus"),
      status: "fail",
      message: `${versionText}. ${t("notInstalled")}.`,
      fixes: [],
    }
  }

  if (
    server.registry_version &&
    server.installed_version !== server.registry_version
  ) {
    return {
      check_id: "version_status",
      label: t("versionStatus"),
      status: "warn",
      message: `${versionText}. ${t("upgradeAvailable")}.`,
      fixes: [],
    }
  }

  if (!server.registry_version) {
    return {
      check_id: "version_status",
      label: t("versionStatus"),
      status: "warn",
      message: `${versionText}. ${t("remoteVersionUnavailable")}.`,
      fixes: [],
    }
  }

  return {
    check_id: "version_status",
    label: t("versionStatus"),
    status: "pass",
    message: `${versionText}. ${t("alreadyLatest")}.`,
    fixes: [],
  }
}

function getServerChecks(
  server: LspServerInfo,
  state: ServerCheckState | undefined,
  t: (key: string) => string
): UiCheckItem[] {
  const versionCheck = buildVersionCheck(server, t)
  const preflightChecks: UiCheckItem[] = (state?.result?.checks ?? []).map(
    (c) => ({ ...c, fixes: [...c.fixes] })
  )
  return [versionCheck, ...preflightChecks]
}

function CheckItemList({
  server,
  checkState,
  expandedChecks,
  onToggleCheck,
  busy,
  runningAction,
  preflightPassed,
  onInstall,
  onUpgrade,
  onUninstall,
}: {
  server: LspServerInfo
  checkState?: ServerCheckState
  expandedChecks: Record<string, boolean>
  onToggleCheck: (checkKey: string) => void
  busy: boolean
  runningAction?: RunningAction
  preflightPassed: boolean
  onInstall: () => void
  onUpgrade: () => void
  onUninstall: () => void
}) {
  const t = useTranslations("LspSettings")

  if (checkState?.error) {
    return (
      <div className="rounded-md bg-destructive/10 p-2 text-xs text-destructive">
        {checkState.error}
      </div>
    )
  }

  const checks = getServerChecks(server, checkState, t)
  if (checks.length === 0) return null

  const isInstalled = !!server.installed_version
  const hasUpdate =
    server.installed_version &&
    server.registry_version &&
    server.installed_version !== server.registry_version

  return (
    <div className="space-y-1.5">
      {checks.map((check) => {
        const checkKey = `${server.id}:${check.check_id}`
        const expanded = expandedChecks[checkKey] ?? check.status !== "pass"
        const isVersionCheck = check.check_id === "version_status"

        return (
          <div
            key={check.check_id}
            className="rounded-md border bg-muted/20 px-3 py-2 space-y-2"
          >
            <button
              type="button"
              className="w-full flex items-center justify-between gap-2 text-left"
              onClick={() => onToggleCheck(checkKey)}
            >
              <div className="min-w-0 flex items-center gap-1.5">
                {expanded ? (
                  <ChevronDown className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                ) : (
                  <ChevronRight className="h-3.5 w-3.5 text-muted-foreground shrink-0" />
                )}
                <span className="text-xs font-medium truncate">
                  {check.label}
                </span>
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
                {/* Version check: install/upgrade/uninstall buttons */}
                {isVersionCheck && (
                  <div className="flex flex-wrap gap-1.5 justify-end shrink-0">
                    {!isInstalled && preflightPassed && (
                      <Button
                        size="xs"
                        variant="outline"
                        className="h-6 bg-muted/30 hover:bg-muted/50"
                        onClick={onInstall}
                        disabled={busy}
                      >
                        {busy && runningAction === "install" ? (
                          <Loader2 className="h-3 w-3 animate-spin" />
                        ) : (
                          <Download className="h-3 w-3" />
                        )}
                        {t("install")}
                      </Button>
                    )}
                    {isInstalled && hasUpdate && (
                      <Button
                        size="xs"
                        variant="outline"
                        className="h-6 bg-muted/30 hover:bg-muted/50"
                        onClick={onUpgrade}
                        disabled={busy}
                      >
                        {busy && runningAction === "upgrade" ? (
                          <Loader2 className="h-3 w-3 animate-spin" />
                        ) : (
                          <RefreshCw className="h-3 w-3" />
                        )}
                        {t("upgrade")}
                      </Button>
                    )}
                    {isInstalled && (
                      <Button
                        size="xs"
                        variant="outline"
                        className="h-6 bg-muted/30 hover:bg-muted/50"
                        onClick={onUninstall}
                        disabled={busy}
                      >
                        {busy && runningAction === "uninstall" ? (
                          <Loader2 className="h-3 w-3 animate-spin" />
                        ) : (
                          <Trash2 className="h-3 w-3" />
                        )}
                        {t("uninstall")}
                      </Button>
                    )}
                  </div>
                )}
                {/* Other checks: fix links */}
                {!isVersionCheck && check.fixes.length > 0 && (
                  <div className="flex flex-wrap gap-1.5 justify-end shrink-0">
                    {check.fixes.map((fix, i) => (
                      <button
                        key={i}
                        className="text-[11px] text-blue-600 dark:text-blue-400 underline"
                        onClick={() => {
                          if (fix.kind === "open_url") {
                            openUrl(fix.payload)
                          }
                        }}
                      >
                        {fix.label}
                      </button>
                    ))}
                  </div>
                )}
              </div>
            )}
          </div>
        )
      })}
    </div>
  )
}
