"use client"

import { useCallback, useEffect, useState } from "react"
import {
  Columns2,
  Loader2,
  MonitorCog,
  RefreshCw,
  SquareTerminal,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  getAvailableTerminalShells,
  getSystemRenderingSettings,
  getSystemTerminalSettings,
  probeTerminalShellPath,
  updateSystemRenderingSettings,
  updateSystemTerminalSettings,
} from "@/lib/api"
import { isDesktop } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import type { AvailableTerminalShells, TerminalShellOption } from "@/lib/types"
import {
  loadLayoutMode,
  saveLayoutMode,
  type WorkspaceLayoutMode,
} from "@/lib/workspace-layout-mode-storage"
import { usePlatform } from "@/hooks/use-platform"
import { relaunchApp } from "@/lib/updater"
import { toErrorMessage } from "@/lib/app-error"
import { DelegationSettingsSection } from "@/components/settings/delegation-settings"

const TERMINAL_SHELL_OPTION_SYSTEM = "system"
const TERMINAL_SHELL_OPTION_CUSTOM = "custom"

/// Pick which dropdown row matches a stored `default_shell` value:
/// - null  → "system"
/// - matches a predefined option's `value` → that option's id
/// - anything else → "custom" (user-supplied path)
function resolveSelectedShellId(
  storedShell: string | null,
  options: TerminalShellOption[]
): string {
  if (!storedShell) return TERMINAL_SHELL_OPTION_SYSTEM
  const matched = options.find(
    (opt) => opt.value !== null && opt.value === storedShell
  )
  return matched?.id ?? TERMINAL_SHELL_OPTION_CUSTOM
}

// Captured the first time the rendering section loads: represents the value
// the running webview process was launched with. Survives settings-shell
// remounts so the "Restart now" banner doesn't vanish if the user navigates
// away and back without restarting.
let processStartDisableHwAccel: boolean | null = null

export function GeneralSettings() {
  const t = useTranslations("GeneralSettings")
  // Backend-driven shell label keys are dynamic strings, so widen `t`
  // for that single call site rather than casting at every use.
  const tDynamic = t as unknown as (key: string) => string
  const { isWindows } = usePlatform()
  const [layoutMode, setLayoutMode] =
    useState<WorkspaceLayoutMode>(loadLayoutMode)

  // Rendering settings are a local Tauri preference (preferences.json). They
  // are only meaningful when the active transport is the local Tauri shell —
  // remote workspace windows route every API call to a remote web server,
  // which deliberately does not expose this endpoint.
  const renderingSettingsLoadable =
    isDesktop() && getActiveRemoteConnectionId() === null
  const renderingSectionVisible = renderingSettingsLoadable && isWindows

  const [loading, setLoading] = useState(true)
  const [loadError, setLoadError] = useState<string | null>(null)

  const [savingTerminal, setSavingTerminal] = useState(false)
  const [availableShells, setAvailableShells] =
    useState<AvailableTerminalShells | null>(null)
  const [selectedShellId, setSelectedShellId] = useState<string>(
    TERMINAL_SHELL_OPTION_SYSTEM
  )
  const [customShellPath, setCustomShellPath] = useState<string>("")
  const [customPathExists, setCustomPathExists] = useState<boolean | null>(null)

  const [disableHwAccel, setDisableHwAccel] = useState(false)
  const [savingRendering, setSavingRendering] = useState(false)
  const [persistedDisableHwAccel, setPersistedDisableHwAccel] = useState(false)
  const [processStartLoaded, setProcessStartLoaded] = useState(
    processStartDisableHwAccel !== null
  )
  const renderingDirty =
    processStartLoaded && persistedDisableHwAccel !== processStartDisableHwAccel

  const loadSettings = useCallback(async () => {
    setLoading(true)
    setLoadError(null)

    try {
      const [terminalSettings, terminalShells, renderingSettings] =
        await Promise.all([
          getSystemTerminalSettings(),
          getAvailableTerminalShells(),
          renderingSettingsLoadable
            ? getSystemRenderingSettings()
            : Promise.resolve(null),
        ])

      setAvailableShells(terminalShells)
      const initialId = resolveSelectedShellId(
        terminalSettings.default_shell,
        terminalShells.options
      )
      setSelectedShellId(initialId)
      if (initialId === TERMINAL_SHELL_OPTION_CUSTOM) {
        setCustomShellPath(terminalSettings.default_shell ?? "")
        setCustomPathExists(
          terminalSettings.default_shell
            ? await probeTerminalShellPath(terminalSettings.default_shell)
            : null
        )
      } else {
        setCustomShellPath("")
        setCustomPathExists(null)
      }

      if (renderingSettings) {
        const value = renderingSettings.disable_hardware_acceleration
        setDisableHwAccel(value)
        setPersistedDisableHwAccel(value)
        if (processStartDisableHwAccel === null) {
          processStartDisableHwAccel = value
          setProcessStartLoaded(true)
        }
      }
    } catch (err) {
      const message = toErrorMessage(err)
      setLoadError(message)
      console.error("[Settings] load general settings failed:", err)
    } finally {
      setLoading(false)
    }
  }, [renderingSettingsLoadable])

  useEffect(() => {
    loadSettings().catch((err) => {
      console.error("[Settings] load general settings failed:", err)
    })
  }, [loadSettings])

  const persistTerminalShell = useCallback(
    async (defaultShell: string | null) => {
      setSavingTerminal(true)
      try {
        const result = await updateSystemTerminalSettings({
          default_shell: defaultShell,
        })
        // Re-fetch options to refresh `exists` flags (e.g. user just installed
        // pwsh, or backend filter dropped a cross-platform stale value).
        const refreshedShells = await getAvailableTerminalShells()
        setAvailableShells(refreshedShells)
        const nextSelectedId = resolveSelectedShellId(
          result.default_shell,
          refreshedShells.options
        )
        setSelectedShellId(nextSelectedId)
        if (nextSelectedId === TERMINAL_SHELL_OPTION_CUSTOM) {
          setCustomShellPath(result.default_shell ?? "")
          setCustomPathExists(
            result.default_shell
              ? await probeTerminalShellPath(result.default_shell)
              : null
          )
        } else {
          setCustomShellPath("")
          setCustomPathExists(null)
        }
      } catch (err) {
        const message = toErrorMessage(err)
        toast.error(t("terminalSaveFailed", { message }))
      } finally {
        setSavingTerminal(false)
      }
    },
    [t]
  )

  const onShellSelectChange = useCallback(
    (nextId: string) => {
      setSelectedShellId(nextId)
      if (nextId === TERMINAL_SHELL_OPTION_CUSTOM) {
        // Don't persist yet — wait for user to type a path and press Save.
        setCustomShellPath("")
        setCustomPathExists(null)
        return
      }
      const matched = availableShells?.options.find((opt) => opt.id === nextId)
      void persistTerminalShell(matched?.value ?? null)
    },
    [availableShells, persistTerminalShell]
  )

  const onCustomPathSave = useCallback(() => {
    const trimmed = customShellPath.trim()
    if (!trimmed) return
    void persistTerminalShell(trimmed)
  }, [customShellPath, persistTerminalShell])

  const saveRenderingSettings = useCallback(
    async (next: boolean, prev: boolean) => {
      setSavingRendering(true)
      try {
        const result = await updateSystemRenderingSettings({
          disable_hardware_acceleration: next,
        })
        setDisableHwAccel(result.disable_hardware_acceleration)
        setPersistedDisableHwAccel(result.disable_hardware_acceleration)
      } catch (err) {
        setDisableHwAccel(prev)
        const message = toErrorMessage(err)
        toast.error(t("renderingSaveFailed", { message }))
      } finally {
        setSavingRendering(false)
      }
    },
    [t]
  )

  const restartNow = useCallback(async () => {
    try {
      await relaunchApp()
    } catch (err) {
      const message = toErrorMessage(err)
      toast.error(t("restartFailed", { message }))
    }
  }, [t])

  const onLayoutModeChange = useCallback((nextMode: WorkspaceLayoutMode) => {
    saveLayoutMode(nextMode)
    setLayoutMode(nextMode)
  }, [])

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center text-sm text-muted-foreground gap-2">
        <Loader2 className="h-4 w-4 animate-spin" />
        {t("loading")}
      </div>
    )
  }

  return (
    <ScrollArea className="h-full">
      <div className="w-full space-y-4 p-3 md:p-4">
        <section className="space-y-1">
          <h1 className="text-sm font-semibold">{t("sectionTitle")}</h1>
          <p className="text-xs text-muted-foreground">
            {t("sectionDescription")}
          </p>
        </section>

        {loadError && (
          <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
            {t("loadFailed", { message: loadError })}
          </div>
        )}

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <Columns2 className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">
              {t("workspaceLayoutTitle")}
            </h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("workspaceLayoutDescription")}
          </p>

          <div className="space-y-2">
            <Select value={layoutMode} onValueChange={onLayoutModeChange}>
              <SelectTrigger className="w-full text-left sm:w-64">
                <SelectValue className="justify-start text-left" />
              </SelectTrigger>
              <SelectContent align="start">
                <SelectItem value="fusion">
                  <div className="flex flex-col items-start text-left">
                    <span>{t("workspaceLayoutFusion")}</span>
                    <span className="text-left text-[10px] text-muted-foreground">
                      {t("workspaceLayoutFusionHint")}
                    </span>
                  </div>
                </SelectItem>
                <SelectItem value="files">
                  <div className="flex flex-col items-start text-left">
                    <span>{t("workspaceLayoutFiles")}</span>
                    <span className="text-left text-[10px] text-muted-foreground">
                      {t("workspaceLayoutFilesHint")}
                    </span>
                  </div>
                </SelectItem>
              </SelectContent>
            </Select>
          </div>
        </section>

        <section className="rounded-xl border bg-card p-4 space-y-4">
          <div className="flex items-center gap-2">
            <SquareTerminal className="h-4 w-4 text-muted-foreground" />
            <h2 className="text-sm font-semibold">{t("terminalTitle")}</h2>
          </div>

          <p className="text-xs text-muted-foreground leading-5">
            {t("terminalDescription")}
          </p>

          <div className="space-y-2">
            <label className="text-xs font-medium text-muted-foreground">
              {t("defaultTerminalShell")}
            </label>
            <Select
              value={selectedShellId}
              onValueChange={onShellSelectChange}
              disabled={savingTerminal || !availableShells}
            >
              <SelectTrigger className="w-full sm:w-64">
                <SelectValue />
              </SelectTrigger>
              <SelectContent align="start">
                {availableShells?.options.map((opt) => (
                  <SelectItem key={opt.id} value={opt.id}>
                    <span className="flex items-center gap-2">
                      <span>{tDynamic(opt.label_key)}</span>
                      {!opt.exists && !opt.accepts_custom_path && (
                        <span className="text-[10px] text-muted-foreground">
                          ({t("terminalShellNotInstalled")})
                        </span>
                      )}
                    </span>
                  </SelectItem>
                ))}
              </SelectContent>
            </Select>
            {availableShells && (
              <p className="text-[11px] text-muted-foreground">
                {t("terminalCurrentShell", {
                  path: availableShells.resolved_shell,
                })}
              </p>
            )}

            {selectedShellId === TERMINAL_SHELL_OPTION_CUSTOM && (
              <div className="space-y-2 pt-2">
                <label className="text-xs font-medium text-muted-foreground">
                  {t("terminalShellCustomPath")}
                </label>
                <div className="flex gap-2">
                  <Input
                    value={customShellPath}
                    onChange={(event) => {
                      setCustomShellPath(event.target.value)
                      setCustomPathExists(null)
                    }}
                    placeholder={t("terminalShellCustomPlaceholder")}
                    disabled={savingTerminal}
                    className="flex-1"
                  />
                  <Button
                    size="sm"
                    onClick={onCustomPathSave}
                    disabled={savingTerminal || !customShellPath.trim()}
                  >
                    {t("terminalShellCustomSave")}
                  </Button>
                </div>
                {customPathExists === false && customShellPath.trim() && (
                  <p className="text-[11px] text-amber-500">
                    {t("terminalShellNotFoundWarning")}
                  </p>
                )}
                <p className="text-[11px] text-muted-foreground">
                  {t("terminalShellCustomHint")}
                </p>
              </div>
            )}
          </div>
        </section>

        {renderingSectionVisible && (
          <section className="rounded-xl border bg-card p-4 space-y-4">
            <div className="flex items-center gap-2">
              <MonitorCog className="h-4 w-4 text-muted-foreground" />
              <h2 className="text-sm font-semibold">{t("renderingTitle")}</h2>
            </div>

            <p className="text-xs text-muted-foreground leading-5">
              {t("renderingDescription")}
            </p>

            <label className="inline-flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={disableHwAccel}
                disabled={savingRendering}
                onChange={(event) => {
                  const next = event.target.checked
                  const prev = disableHwAccel
                  setDisableHwAccel(next)
                  saveRenderingSettings(next, prev)
                }}
              />
              {t("disableHardwareAcceleration")}
            </label>

            {renderingDirty && (
              <div className="flex items-center justify-between gap-3 rounded-md border bg-muted/20 px-3 py-2 text-xs">
                <span className="text-muted-foreground">
                  {t("restartRequired")}
                </span>
                <Button
                  size="sm"
                  onClick={restartNow}
                  disabled={savingRendering}
                >
                  <RefreshCw className="h-3.5 w-3.5" />
                  {t("restartNow")}
                </Button>
              </div>
            )}
          </section>
        )}

        <DelegationSettingsSection />
      </div>
    </ScrollArea>
  )
}
