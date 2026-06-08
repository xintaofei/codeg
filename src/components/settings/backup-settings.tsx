"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import {
  DatabaseBackup,
  Download,
  Loader2,
  ShieldAlert,
  Upload,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Switch } from "@/components/ui/switch"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog"
import { isDesktop } from "@/lib/platform"
import { getActiveRemoteConnectionId } from "@/lib/transport"
import { relaunchApp, restartApp, waitForServerHealthy } from "@/lib/updater"
import {
  toLocalizedErrorMessage,
  type AppErrorTranslator,
} from "@/lib/app-error"
import {
  exportBackupDesktop,
  exportBackupWeb,
  inspectBackupDesktop,
  inspectBackupWeb,
  listenBackupProgress,
  scanExternalConflictsDesktop,
  scanExternalConflictsWeb,
  stageRestoreDesktop,
  stageRestoreWeb,
  uploadBackupWeb,
  type BackupPreview,
  type BackupProgress,
  type ExternalConflict,
  type ExternalRestoreMode,
} from "@/lib/api"

type RestoreSource =
  | { kind: "desktop"; path: string; name: string }
  | { kind: "web"; uploadId: string; name: string }

type ExternalChoice = "skip" | "side" | "original"

const ACTIVE_PHASES: BackupProgress["phase"][] = [
  "snapshotting",
  "archiving",
  "encrypting",
  "decrypting",
  "extracting",
  "verifying",
  "swapping",
]

function formatMb(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

export function BackupSettings() {
  const t = useTranslations("BackupSettings")
  // Root-scoped translator so backend errors carrying dotted i18n keys
  // (`backup.restore.error.*`) localize; falls back to the English message when
  // a key is absent. next-intl's typed `t` is widened to the loose translator
  // shape `toLocalizedErrorMessage` expects.
  const tRoot = useTranslations()
  const localize = useCallback(
    (err: unknown) =>
      toLocalizedErrorMessage(err, tRoot as unknown as AppErrorTranslator),
    [tRoot]
  )
  // A remote-desktop window is a Tauri shell whose transport points at a remote
  // server: native dialogs + local Tauri commands would not line up with that
  // server's ticket/upload web API, so backup is managed on the server itself.
  const remote = isDesktop() && getActiveRemoteConnectionId() !== null
  // "desktop" path = local Tauri only (native dialogs + Tauri commands). Both
  // standalone web and remote-desktop use the web flow / are gated below.
  const desktop = isDesktop() && getActiveRemoteConnectionId() === null

  // ── Export ──
  const [includeExternal, setIncludeExternal] = useState(false)
  const [passphrase, setPassphrase] = useState("")
  const [passphraseConfirm, setPassphraseConfirm] = useState("")
  const [exporting, setExporting] = useState(false)

  // ── Restore ──
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [restoreSource, setRestoreSource] = useState<RestoreSource | null>(null)
  const [preview, setPreview] = useState<BackupPreview | null>(null)
  const [restorePassphrase, setRestorePassphrase] = useState("")
  const [inspecting, setInspecting] = useState(false)
  const [uploading, setUploading] = useState(false)
  const [confirmOpen, setConfirmOpen] = useState(false)
  const [restoring, setRestoring] = useState(false)

  // ── External transcripts (opt-in restore) ──
  const [externalChoice, setExternalChoice] = useState<ExternalChoice>("skip")
  const [forceOverwrite, setForceOverwrite] = useState(false)
  const [conflicts, setConflicts] = useState<ExternalConflict[] | null>(null)
  const [scanningConflicts, setScanningConflicts] = useState(false)

  // ── Shared progress feed ──
  const [progress, setProgress] = useState<BackupProgress | null>(null)
  useEffect(() => {
    let active = true
    let unsub: (() => void) | undefined
    void listenBackupProgress((event) => setProgress(event)).then((fn) => {
      if (active) unsub = fn
      else fn()
    })
    return () => {
      active = false
      unsub?.()
    }
  }, [])

  const passphraseMismatch =
    passphrase.length > 0 && passphrase !== passphraseConfirm
  const busy = exporting || restoring

  const resetExternalState = useCallback(() => {
    setExternalChoice("skip")
    setForceOverwrite(false)
    setConflicts(null)
  }, [])

  const handleExport = useCallback(async () => {
    if (passphraseMismatch) {
      toast.error(t("export.passphraseMismatch"))
      return
    }
    setExporting(true)
    setProgress(null)
    try {
      const opts = {
        includeExternalTranscripts: includeExternal,
        passphrase: passphrase || null,
      }
      if (desktop) {
        const manifest = await exportBackupDesktop(opts)
        if (manifest) toast.success(t("export.success"))
      } else {
        await exportBackupWeb(opts)
        toast.success(t("export.started"))
      }
    } catch (err) {
      toast.error(localize(err))
    } finally {
      setExporting(false)
      setProgress(null)
    }
  }, [desktop, includeExternal, passphrase, passphraseMismatch, t, localize])

  const runInspect = useCallback(
    async (source: RestoreSource, pass: string | null) => {
      setInspecting(true)
      try {
        const pv =
          source.kind === "desktop"
            ? await inspectBackupDesktop(source.path, pass)
            : await inspectBackupWeb(source.uploadId, pass)
        setPreview(pv)
      } catch (err) {
        toast.error(localize(err))
        setPreview(null)
      } finally {
        setInspecting(false)
      }
    },
    [localize]
  )

  const handlePickDesktop = useCallback(async () => {
    const { open } = await import("@tauri-apps/plugin-dialog")
    const picked = await open({
      multiple: false,
      filters: [{ name: "Codeg backup", extensions: ["codegbak", "zip"] }],
    })
    if (typeof picked !== "string") return
    const name = picked.split(/[\\/]/).pop() ?? picked
    const source: RestoreSource = { kind: "desktop", path: picked, name }
    setRestoreSource(source)
    setPreview(null)
    setRestorePassphrase("")
    resetExternalState()
    await runInspect(source, null)
  }, [runInspect, resetExternalState])

  const handlePickWeb = useCallback(
    async (file: File) => {
      setRestoreSource(null)
      setPreview(null)
      setRestorePassphrase("")
      resetExternalState()
      setUploading(true)
      try {
        const uploadId = await uploadBackupWeb(file)
        const source: RestoreSource = {
          kind: "web",
          uploadId,
          name: file.name,
        }
        setRestoreSource(source)
        await runInspect(source, null)
      } catch (err) {
        toast.error(localize(err))
      } finally {
        setUploading(false)
      }
    },
    [runInspect, resetExternalState, localize]
  )

  const handleUnlock = useCallback(async () => {
    if (!restoreSource) return
    await runInspect(restoreSource, restorePassphrase || null)
  }, [restoreSource, restorePassphrase, runInspect])

  const hasExternal = !!preview?.manifest?.includesExternalTranscripts

  const buildExternalMode = useCallback((): ExternalRestoreMode | null => {
    if (!hasExternal) return null
    if (externalChoice === "skip") return { mode: "skip" }
    if (externalChoice === "side") return { mode: "side_location" }
    return {
      mode: "original_locations",
      on_conflict: forceOverwrite ? "overwrite" : "skip_existing",
    }
  }, [hasExternal, externalChoice, forceOverwrite])

  const handleExternalChoice = useCallback(
    async (choice: ExternalChoice) => {
      setExternalChoice(choice)
      setConflicts(null)
      if (choice !== "original" || !restoreSource) return
      setScanningConflicts(true)
      try {
        const found =
          restoreSource.kind === "desktop"
            ? await scanExternalConflictsDesktop(
                restoreSource.path,
                restorePassphrase || null
              )
            : await scanExternalConflictsWeb(
                restoreSource.uploadId,
                restorePassphrase || null
              )
        setConflicts(found)
      } catch (err) {
        toast.error(localize(err))
      } finally {
        setScanningConflicts(false)
      }
    },
    [restoreSource, restorePassphrase, localize]
  )

  const performRestore = useCallback(async () => {
    if (!restoreSource) return
    setConfirmOpen(false)
    setRestoring(true)
    setProgress(null)
    try {
      const pass = restorePassphrase || null
      const externalMode = buildExternalMode()
      if (restoreSource.kind === "desktop") {
        await stageRestoreDesktop({
          srcPath: restoreSource.path,
          passphrase: pass,
          externalMode,
        })
        toast.success(t("restore.staged"))
        await relaunchApp()
      } else {
        const res = await stageRestoreWeb({
          uploadId: restoreSource.uploadId,
          passphrase: pass,
          externalMode,
        })
        if (res.staged.restoredExternalPath) {
          toast.message(
            t("restore.externalSideLocation", {
              path: res.staged.restoredExternalPath,
            })
          )
        }
        // The restore is staged but only APPLIED on the next server start. If
        // the restart request fails (e.g. unsupported platform, busy), do NOT
        // poll health + reload — that would land back on the still-running old
        // process and look like success while the restore never applied. Tell
        // the user to restart manually instead.
        try {
          await restartApp()
        } catch {
          toast.error(t("restore.restartFailed"))
          setRestoring(false)
          return
        }
        toast.success(t("restore.restarting"))
        const healthy = await waitForServerHealthy({
          timeoutMs: 120_000,
          initialDelayMs: 1500,
        })
        if (healthy) window.location.reload()
        else {
          toast.error(t("restore.restartTimeout"))
          setRestoring(false)
        }
      }
    } catch (err) {
      toast.error(localize(err))
      setRestoring(false)
    }
  }, [restoreSource, restorePassphrase, buildExternalMode, t, localize])

  const showProgress = progress && ACTIVE_PHASES.includes(progress.phase)

  // Embedded as a card inside the System settings page; the page owns the
  // outer scroll + padding, so this renders a self-contained section.
  if (remote) {
    return (
      <section className="rounded-xl border bg-card p-4 space-y-4">
        <div className="flex items-center gap-2">
          <DatabaseBackup className="h-4 w-4" />
          <h2 className="text-sm font-semibold">{t("title")}</h2>
        </div>
        <p className="text-xs text-muted-foreground">{t("description")}</p>
        <div className="rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-xs text-amber-500">
          {t("remoteUnsupported")}
        </div>
      </section>
    )
  }

  return (
    <>
      <section className="rounded-xl border bg-card p-4 space-y-4">
        <div className="flex items-center gap-2">
          <DatabaseBackup className="h-4 w-4" />
          <h2 className="text-sm font-semibold">{t("title")}</h2>
        </div>
        <p className="text-xs text-muted-foreground">{t("description")}</p>

        <Tabs defaultValue="backup">
          <TabsList className="w-full">
            <TabsTrigger value="backup" className="flex-1" disabled={busy}>
              <Download className="h-3.5 w-3.5" />
              {t("tabs.backup")}
            </TabsTrigger>
            <TabsTrigger value="restore" className="flex-1" disabled={busy}>
              <Upload className="h-3.5 w-3.5" />
              {t("tabs.restore")}
            </TabsTrigger>
          </TabsList>

          {/* ── Backup ── */}
          <TabsContent value="backup" className="space-y-4 pt-2">
            <div className="flex items-center justify-between gap-3">
              <div className="space-y-0.5">
                <Label className="text-xs font-medium">
                  {t("export.includeExternal")}
                </Label>
                <p className="text-[11px] text-muted-foreground">
                  {t("export.includeExternalHint")}
                </p>
              </div>
              <Switch
                checked={includeExternal}
                onCheckedChange={setIncludeExternal}
                disabled={busy}
              />
            </div>

            <div className="space-y-2">
              <Label className="text-xs font-medium">
                {t("export.passphrase")}
              </Label>
              <Input
                type="password"
                autoComplete="new-password"
                value={passphrase}
                onChange={(e) => setPassphrase(e.target.value)}
                placeholder={t("export.passphrasePlaceholder")}
                disabled={busy}
              />
              {passphrase.length > 0 && (
                <Input
                  type="password"
                  autoComplete="new-password"
                  value={passphraseConfirm}
                  onChange={(e) => setPassphraseConfirm(e.target.value)}
                  placeholder={t("export.passphraseConfirm")}
                  disabled={busy}
                />
              )}
              {passphraseMismatch && (
                <p className="text-[11px] text-red-400">
                  {t("export.passphraseMismatch")}
                </p>
              )}
              {passphrase.length === 0 ? (
                <div className="flex items-start gap-2 rounded-md border border-amber-500/30 bg-amber-500/5 px-3 py-2 text-[11px] text-amber-500">
                  <ShieldAlert className="h-3.5 w-3.5 mt-0.5 shrink-0" />
                  <span>{t("export.noPassphraseWarning")}</span>
                </div>
              ) : (
                <p className="text-[11px] text-muted-foreground">
                  {t("export.passphraseLossWarning")}
                </p>
              )}
            </div>

            <Button
              type="button"
              size="sm"
              onClick={handleExport}
              disabled={busy || passphraseMismatch}
            >
              {exporting && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
              {t("export.button")}
            </Button>

            {exporting && showProgress && (
              <ProgressLine
                progress={progress}
                label={t("export.inProgress")}
              />
            )}
          </TabsContent>

          {/* ── Restore ── */}
          <TabsContent value="restore" className="space-y-4 pt-2">
            <div className="flex flex-wrap items-center gap-2">
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={busy || inspecting || uploading}
                onClick={() => {
                  if (desktop) void handlePickDesktop()
                  else fileInputRef.current?.click()
                }}
              >
                {(inspecting || uploading) && (
                  <Loader2 className="h-3.5 w-3.5 animate-spin" />
                )}
                {t("restore.selectFile")}
              </Button>
              {restoreSource && (
                <span className="text-xs text-muted-foreground truncate">
                  {restoreSource.name}
                </span>
              )}
              {!desktop && (
                <input
                  ref={fileInputRef}
                  type="file"
                  accept=".codegbak,.zip,application/zip"
                  className="hidden"
                  onChange={(e) => {
                    const file = e.target.files?.[0]
                    if (file) void handlePickWeb(file)
                    e.target.value = ""
                  }}
                />
              )}
            </div>

            {preview?.needsPassphrase && (
              <div className="space-y-2">
                <Label className="text-xs font-medium">
                  {t("restore.passphrasePrompt")}
                </Label>
                <div className="flex gap-2">
                  <Input
                    type="password"
                    value={restorePassphrase}
                    onChange={(e) => setRestorePassphrase(e.target.value)}
                    disabled={inspecting}
                  />
                  <Button
                    type="button"
                    size="sm"
                    variant="secondary"
                    onClick={handleUnlock}
                    disabled={inspecting || restorePassphrase.length === 0}
                  >
                    {t("restore.unlock")}
                  </Button>
                </div>
              </div>
            )}

            {preview?.manifest && (
              <div className="rounded-md border bg-muted/20 p-3 text-xs space-y-1.5">
                <div className="flex items-center gap-2">
                  <span className="font-medium">
                    {t("restore.preview.title")}
                  </span>
                  {preview.encrypted && (
                    <Badge variant="secondary">
                      {t("restore.preview.encrypted")}
                    </Badge>
                  )}
                  {preview.compatible ? (
                    <Badge variant="outline">
                      {t("restore.preview.compatible")}
                    </Badge>
                  ) : (
                    <Badge variant="destructive">
                      {t("restore.preview.incompatible")}
                    </Badge>
                  )}
                </div>
                <div className="text-muted-foreground">
                  {t("restore.preview.createdAt", {
                    value: new Date(
                      preview.manifest.createdAt
                    ).toLocaleString(),
                  })}
                </div>
                <div className="text-muted-foreground">
                  {t("restore.preview.appVersion", {
                    value: preview.manifest.appVersion,
                  })}
                </div>
                {!preview.compatible && (
                  <div className="text-red-400">
                    {t("restore.preview.incompatibleHint")}
                  </div>
                )}
              </div>
            )}

            {hasExternal && preview?.compatible && (
              <div className="space-y-2 rounded-md border bg-muted/20 p-3">
                <div className="space-y-0.5">
                  <Label className="text-xs font-medium">
                    {t("restore.external.title")}
                  </Label>
                  <p className="text-[11px] text-muted-foreground">
                    {t("restore.external.hint")}
                  </p>
                </div>
                <Select
                  value={externalChoice}
                  onValueChange={(v) =>
                    void handleExternalChoice(v as ExternalChoice)
                  }
                >
                  <SelectTrigger className="h-8 text-xs">
                    <SelectValue />
                  </SelectTrigger>
                  <SelectContent>
                    <SelectItem value="skip">
                      {t("restore.external.modeSkip")}
                    </SelectItem>
                    <SelectItem value="side">
                      {t("restore.external.modeSide")}
                    </SelectItem>
                    <SelectItem value="original">
                      {t("restore.external.modeOriginal")}
                    </SelectItem>
                  </SelectContent>
                </Select>
                {externalChoice === "original" && (
                  <div className="space-y-2">
                    {scanningConflicts ? (
                      <p className="text-[11px] text-muted-foreground">
                        {t("restore.external.scanning")}
                      </p>
                    ) : conflicts && conflicts.length === 0 ? (
                      <p className="text-[11px] text-muted-foreground">
                        {t("restore.external.noConflicts")}
                      </p>
                    ) : conflicts && conflicts.length > 0 ? (
                      <div className="space-y-1">
                        <p className="text-[11px] text-amber-500">
                          {t("restore.external.conflictCount", {
                            count: conflicts.length,
                          })}
                        </p>
                        <p className="text-[11px] text-muted-foreground">
                          {t("restore.external.conflictSkipNote")}
                        </p>
                      </div>
                    ) : null}
                    <div className="flex items-center justify-between gap-3">
                      <div className="space-y-0.5">
                        <Label className="text-xs font-medium">
                          {t("restore.external.forceOverwrite")}
                        </Label>
                        <p className="text-[11px] text-muted-foreground">
                          {t("restore.external.forceOverwriteHint")}
                        </p>
                      </div>
                      <Switch
                        checked={forceOverwrite}
                        onCheckedChange={setForceOverwrite}
                        disabled={busy}
                      />
                    </div>
                  </div>
                )}
              </div>
            )}

            {preview?.manifest && (
              <div className="flex items-start gap-2 rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-[11px] text-red-400">
                <ShieldAlert className="h-3.5 w-3.5 mt-0.5 shrink-0" />
                <span>
                  {t("restore.replaceWarning")}
                  {/* Keyring note applies to local desktop only: GitHub/chat
                      tokens live in the OS keychain and are NOT in the backup,
                      so they need re-entry after a desktop restore. In
                      server/web mode those tokens are in tokens.json, which IS
                      backed up. */}
                  {desktop ? ` ${t("restore.keyringNote")}` : ""}
                </span>
              </div>
            )}

            <Button
              type="button"
              size="sm"
              variant="destructive"
              disabled={
                busy || !preview?.manifest || !preview.compatible || inspecting
              }
              onClick={() => setConfirmOpen(true)}
            >
              {restoring && <Loader2 className="h-3.5 w-3.5 animate-spin" />}
              {t("restore.button")}
            </Button>

            {restoring && showProgress && (
              <ProgressLine progress={progress} label={t("restore.staging")} />
            )}
          </TabsContent>
        </Tabs>
      </section>

      <AlertDialog open={confirmOpen} onOpenChange={setConfirmOpen}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("restore.confirmTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("restore.confirmBody")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("restore.cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={performRestore}>
              {t("restore.confirmAction")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}

function ProgressLine({
  progress,
  label,
}: {
  progress: BackupProgress
  label: string
}) {
  const pct =
    progress.totalBytes && progress.totalBytes > 0
      ? Math.min(100, (progress.processedBytes / progress.totalBytes) * 100)
      : null
  return (
    <div className="space-y-1">
      <div className="flex items-center justify-between text-[11px] text-muted-foreground">
        <span>{label}</span>
        <span>{formatMb(progress.processedBytes)}</span>
      </div>
      <div className="h-1.5 w-full overflow-hidden rounded-full bg-muted">
        <div
          className={
            pct === null
              ? "h-full w-1/3 animate-pulse bg-primary"
              : "h-full bg-primary transition-all"
          }
          style={pct === null ? undefined : { width: `${pct}%` }}
        />
      </div>
    </div>
  )
}
