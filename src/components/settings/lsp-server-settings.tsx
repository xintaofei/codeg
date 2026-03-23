"use client"

import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type PointerEvent,
} from "react"
import { Reorder, useDragControls } from "motion/react"
import { useTranslations } from "next-intl"
import {
  AlertCircle,
  CheckCircle2,
  ChevronDown,
  ChevronRight,
  Download,
  GripVertical,
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
  lspReorderServers,
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

const LANGUAGE_COLORS: Record<string, string> = {
  "TypeScript/JavaScript": "bg-blue-500",
  "HTML/CSS/JSON": "bg-orange-500",
  Shell: "bg-green-500",
  YAML: "bg-yellow-500",
  Python: "bg-sky-500",
  Rust: "bg-amber-600",
  Go: "bg-cyan-500",
  "C/C++": "bg-purple-500",
  Lua: "bg-indigo-500",
  TOML: "bg-rose-500",
}

interface ServerCheckState {
  result?: LspPreflightResult
  error?: string
}

type RunningAction =
  | "install"
  | "upgrade"
  | "uninstall"
  | "detect_version"

export function LspServerSettings() {
  const t = useTranslations("LspSettings")
  const [servers, setServers] = useState<LspServerInfo[]>([])
  const [loading, setLoading] = useState(true)
  const [loadingError, setLoadingError] = useState<string | null>(null)
  const [checkState, setCheckState] = useState<Record<string, ServerCheckState>>(
    {}
  )
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
  const [dragging, setDragging] = useState<string | null>(null)
  const [reordering, setReordering] = useState(false)
  const pendingOrderRef = useRef<string[] | null>(null)

  const sortedServers = useMemo(
    () =>
      [...servers].sort(
        (a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name)
      ),
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

  useEffect(() => {
    refreshServers()
  }, [refreshServers])

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
            s.id === serverId
              ? { ...s, installed_version: detectedVersion }
              : s
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
        await refreshServers()
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
    [refreshServers, t]
  )

  const handleToggleEnabled = useCallback(
    async (server: LspServerInfo, enabled: boolean) => {
      try {
        await lspUpdateServerPreferences(
          server.id,
          enabled,
          server.config_json
        )
        setServers((prev) =>
          prev.map((s) => (s.id === server.id ? { ...s, enabled } : s))
        )
      } catch (err) {
        toast.error(String(err))
      }
    },
    []
  )

  const handleReorder = useCallback(
    (newOrder: LspServerInfo[]) => {
      setServers(newOrder)
      pendingOrderRef.current = newOrder.map((s) => s.id)
    },
    []
  )

  const flushReorder = useCallback(async () => {
    const ids = pendingOrderRef.current
    if (!ids) return
    pendingOrderRef.current = null
    setReordering(true)
    try {
      await lspReorderServers(ids)
    } catch {
      // Revert on failure
      await refreshServers()
    } finally {
      setReordering(false)
    }
  }, [refreshServers])

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
      {/* Server list */}
      <div className="w-full flex flex-col">
        <div className="px-4 pt-3 pb-2 flex items-center justify-between">
          <h2 className="text-sm font-semibold">{t("title")}</h2>
          <Button
            variant="ghost"
            size="icon-sm"
            onClick={refreshServers}
            disabled={reordering}
          >
            <RefreshCw className="h-3.5 w-3.5" />
          </Button>
        </div>

        <div className="flex-1 overflow-y-auto px-4 pb-4">
          <Reorder.Group
            axis="y"
            values={sortedServers}
            onReorder={handleReorder}
            className="space-y-1"
          >
            {sortedServers.map((server) => (
              <LspServerRow
                key={server.id}
                server={server}
                selected={selectedId === server.id}
                onSelect={() =>
                  setSelectedId((prev) =>
                    prev === server.id ? null : server.id
                  )
                }
                checking={!!checking[server.id]}
                checkState={checkState[server.id]}
                busy={!!busyAction[server.id]}
                runningAction={runningAction[server.id]}
                expandedChecks={!!expandedChecks[server.id]}
                onToggleChecks={() =>
                  setExpandedChecks((prev) => ({
                    ...prev,
                    [server.id]: !prev[server.id],
                  }))
                }
                onRunPreflight={() => runPreflight(server.id)}
                onInstall={() => handleInstall(server)}
                onUpgrade={() => handleUpgrade(server)}
                onUninstall={() => setUninstallConfirm(server)}
                onToggleEnabled={(enabled) =>
                  handleToggleEnabled(server, enabled)
                }
                onDragStart={() => setDragging(server.id)}
                onDragEnd={() => {
                  setDragging(null)
                  flushReorder()
                }}
                dragging={dragging === server.id}
              />
            ))}
          </Reorder.Group>
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

// ─── Server Row ──────────────────────────────────────────────────────────

interface LspServerRowProps {
  server: LspServerInfo
  selected: boolean
  onSelect: () => void
  checking: boolean
  checkState?: ServerCheckState
  busy: boolean
  runningAction?: RunningAction
  expandedChecks: boolean
  onToggleChecks: () => void
  onRunPreflight: () => void
  onInstall: () => void
  onUpgrade: () => void
  onUninstall: () => void
  onToggleEnabled: (enabled: boolean) => void
  onDragStart: () => void
  onDragEnd: () => void
  dragging: boolean
}

function LspServerRow({
  server,
  selected,
  onSelect,
  checking,
  checkState,
  busy,
  runningAction,
  expandedChecks,
  onToggleChecks,
  onRunPreflight,
  onInstall,
  onUpgrade,
  onUninstall,
  onToggleEnabled,
  onDragStart,
  onDragEnd,
  dragging,
}: LspServerRowProps) {
  const t = useTranslations("LspSettings")
  const dragControls = useDragControls()

  const hasUpdate =
    server.installed_version &&
    server.registry_version &&
    server.installed_version !== server.registry_version

  const isInstalled = !!server.installed_version

  return (
    <Reorder.Item
      value={server}
      dragControls={dragControls}
      dragListener={false}
      onDragEnd={onDragEnd}
      className={cn(
        "rounded-lg border bg-card",
        dragging && "opacity-60",
        selected && "ring-1 ring-ring"
      )}
    >
      {/* Header row */}
      <div className="flex items-center gap-2 p-3">
        <button
          className="touch-none cursor-grab active:cursor-grabbing text-muted-foreground hover:text-foreground"
          onPointerDown={(e: PointerEvent) => {
            onDragStart()
            dragControls.start(e)
          }}
        >
          <GripVertical className="h-4 w-4" />
        </button>

        <button
          className="flex-1 flex items-center gap-2 text-left min-w-0"
          onClick={onSelect}
        >
          <span
            className={cn(
              "inline-block h-2.5 w-2.5 rounded-full shrink-0",
              LANGUAGE_COLORS[server.language] ?? "bg-gray-400"
            )}
          />
          <span className="text-sm font-medium truncate">{server.name}</span>
          <Badge variant="outline" className="text-[10px] px-1.5 py-0 shrink-0">
            {DISTRIBUTION_LABELS[server.distribution_type] ??
              server.distribution_type}
          </Badge>
          {isInstalled && (
            <Badge
              variant="secondary"
              className="text-[10px] px-1.5 py-0 shrink-0"
            >
              v{server.installed_version}
            </Badge>
          )}
          {hasUpdate && (
            <Badge className="text-[10px] px-1.5 py-0 bg-blue-500/15 text-blue-600 dark:text-blue-400 border-blue-500/25 shrink-0">
              {t("updateAvailable")}
            </Badge>
          )}
        </button>

        <Switch
          checked={server.enabled}
          onCheckedChange={(checked) => onToggleEnabled(checked)}
          className="shrink-0"
        />
      </div>

      {/* Expanded detail */}
      <Collapsible open={selected}>
        <CollapsibleContent>
          <div className="px-3 pb-3 space-y-3 border-t pt-3">
            <p className="text-xs text-muted-foreground">
              {server.description}
            </p>

            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <span>
                {t("language")}: {server.language}
              </span>
              {server.registry_version && (
                <>
                  <span className="text-border">|</span>
                  <span>
                    {t("latestVersion")}: {server.registry_version}
                  </span>
                </>
              )}
            </div>

            {/* Action buttons */}
            <div className="flex items-center gap-2 flex-wrap">
              {!isInstalled && (
                <Button
                  size="xs"
                  onClick={onInstall}
                  disabled={busy}
                >
                  {busy && runningAction === "install" ? (
                    <Loader2 className="mr-1.5 h-3 w-3 animate-spin" />
                  ) : (
                    <Download className="mr-1.5 h-3 w-3" />
                  )}
                  {t("install")}
                </Button>
              )}

              {isInstalled && hasUpdate && (
                <Button
                  size="xs"
                  onClick={onUpgrade}
                  disabled={busy}
                >
                  {busy && runningAction === "upgrade" ? (
                    <Loader2 className="mr-1.5 h-3 w-3 animate-spin" />
                  ) : (
                    <RefreshCw className="mr-1.5 h-3 w-3" />
                  )}
                  {t("upgrade")}
                </Button>
              )}

              {isInstalled && (
                <Button
                  size="xs"
                  variant="destructive"
                  onClick={onUninstall}
                  disabled={busy}
                >
                  {busy && runningAction === "uninstall" ? (
                    <Loader2 className="mr-1.5 h-3 w-3 animate-spin" />
                  ) : (
                    <Trash2 className="mr-1.5 h-3 w-3" />
                  )}
                  {t("uninstall")}
                </Button>
              )}

              <Button
                size="xs"
                variant="outline"
                onClick={onRunPreflight}
                disabled={checking || busy}
              >
                {checking ? (
                  <Loader2 className="mr-1.5 h-3 w-3 animate-spin" />
                ) : (
                  <CheckCircle2 className="mr-1.5 h-3 w-3" />
                )}
                {t("preflight")}
              </Button>
            </div>

            {/* Preflight results */}
            {checkState && (
              <PreflightSection
                state={checkState}
                expanded={expandedChecks}
                onToggle={onToggleChecks}
              />
            )}
          </div>
        </CollapsibleContent>
      </Collapsible>
    </Reorder.Item>
  )
}

// ─── Preflight Section ───────────────────────────────────────────────────

function PreflightSection({
  state,
  expanded,
  onToggle,
}: {
  state: ServerCheckState
  expanded: boolean
  onToggle: () => void
}) {
  const t = useTranslations("LspSettings")

  if (state.error) {
    return (
      <div className="rounded-md bg-destructive/10 p-2 text-xs text-destructive">
        {state.error}
      </div>
    )
  }

  if (!state.result) return null

  const { passed, checks } = state.result
  const Icon = expanded ? ChevronDown : ChevronRight

  return (
    <div className="space-y-1">
      <button
        className="flex items-center gap-1.5 text-xs font-medium"
        onClick={onToggle}
      >
        <Icon className="h-3 w-3" />
        {passed ? (
          <span className="text-green-600 dark:text-green-400">
            {t("preflightPassed")}
          </span>
        ) : (
          <span className="text-destructive">{t("preflightFailed")}</span>
        )}
      </button>

      {expanded && (
        <div className="space-y-1 ml-4">
          {checks.map((check) => (
            <CheckItemRow key={check.check_id} check={check} />
          ))}
        </div>
      )}
    </div>
  )
}

function CheckItemRow({
  check,
}: {
  check: { check_id: string; label: string; status: CheckStatus; message: string; fixes: FixAction[] }
}) {
  const statusIcon =
    check.status === "pass" ? (
      <CheckCircle2 className="h-3 w-3 text-green-600 dark:text-green-400" />
    ) : check.status === "fail" ? (
      <AlertCircle className="h-3 w-3 text-destructive" />
    ) : (
      <AlertCircle className="h-3 w-3 text-yellow-500" />
    )

  return (
    <div className="flex items-start gap-1.5 text-xs">
      <span className="mt-0.5 shrink-0">{statusIcon}</span>
      <div className="min-w-0">
        <span className="font-medium">{check.label}</span>
        <span className="text-muted-foreground"> — {check.message}</span>
        {check.fixes.length > 0 && (
          <div className="mt-0.5 flex gap-2">
            {check.fixes.map((fix, i) => (
              <button
                key={i}
                className="text-blue-600 dark:text-blue-400 underline"
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
    </div>
  )
}
