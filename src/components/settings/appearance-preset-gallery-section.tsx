"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useLocale, useTranslations } from "next-intl"
import { useTheme } from "next-themes"
import { toast } from "sonner"
import {
  Check,
  Download,
  Eye,
  Loader2,
  RefreshCw,
  Settings2,
  Store,
  Trash2,
} from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { useAppearancePresets, useCustomStyle } from "@/hooks/use-appearance"
import {
  toLocalizedErrorMessage,
  type AppErrorTranslator,
} from "@/lib/app-error"
import {
  MAX_PRESET_BYTES,
  type AppearancePreset,
  type PresetIssue,
} from "@/lib/appearance-preset"
import {
  DEFAULT_PRESET_GALLERY_INDEX_URL,
  MAX_GALLERY_INDEX_BYTES,
  downloadGalleryPreset,
  filterGalleryEntries,
  galleryEntryStatus,
  galleryEntrySwatches,
  installedFromDownload,
  isGalleryCacheStale,
  normalizeGalleryIndexUrl,
  parsePresetGalleryIndex,
  presetSwatches,
  readGalleryCache,
  readGalleryIndexUrl,
  readInstalledGalleryPresets,
  refreshGalleryIndex,
  removeInstalledGalleryPreset,
  upsertInstalledGalleryPreset,
  writeGalleryIndexUrl,
  writeInstalledGalleryPresets,
  type GalleryEntry,
  type GalleryIndex,
  type GalleryIssue,
  type InstalledGalleryPreset,
} from "@/lib/appearance-preset-gallery"
import { STORAGE_KEY_PRESET_GALLERY_INSTALLED } from "@/lib/appearance-script"
import { cn } from "@/lib/utils"

type GalleryState = {
  index: GalleryIndex | null
  skipped: GalleryIssue[]
  /** ISO timestamp of the fetch the index on screen came from. */
  fetchedAt: string | null
  /** The index on screen is the saved copy; no fetch has succeeded yet. */
  fromCache: boolean
}

type LoadError =
  | { kind: "index"; issues: GalleryIssue[] }
  | { kind: "transport"; error: unknown }

type FileError = {
  name: string
  code: "hash-mismatch" | "id-mismatch" | "invalid-preset" | "transport"
  issues?: PresetIssue[]
  error?: unknown
}

type Preview = {
  entry: GalleryEntry
  preset: AppearancePreset
  sha256: string
  /** The next-themes setting before the preview, restored on revert. */
  previousTheme: string | undefined
}

const EMPTY_GALLERY: GalleryState = {
  index: null,
  skipped: [],
  fetchedAt: null,
  fromCache: false,
}

/** The saved copy for `indexUrl`, parsed; empty when there is none or it no longer validates. */
function readSavedGallery(indexUrl: string): GalleryState {
  const cache = readGalleryCache(indexUrl)
  if (!cache) return EMPTY_GALLERY
  const parsed = parsePresetGalleryIndex(cache.text, indexUrl)
  if (!parsed.ok) return EMPTY_GALLERY
  return {
    index: parsed.index,
    skipped: parsed.skipped,
    fetchedAt: cache.fetchedAt,
    fromCache: true,
  }
}

function syncAppearanceMode(mode: string) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window))
    return
  import("@/lib/tauri").then((t) =>
    t.updateAppearanceMode(mode).catch(() => {})
  )
}

function SwatchStrip({ colors }: { colors: string[] }) {
  return (
    <span className="flex items-center gap-1.5" aria-hidden>
      {colors.map((color, index) => (
        <span
          key={index}
          className="size-4 rounded-full border"
          style={{ backgroundColor: color }}
        />
      ))}
    </span>
  )
}

export function AppearancePresetGallerySection() {
  const t = useTranslations("AppearanceSettings")
  const locale = useLocale()
  const { theme, setTheme } = useTheme()
  const {
    appliedPreset,
    applyPreset,
    presetPreview,
    startPresetPreview,
    endPresetPreview,
  } = useAppearancePresets()
  const { isDarkMode } = useCustomStyle()

  const [indexUrl, setIndexUrl] = useState(() => readGalleryIndexUrl())
  const [gallery, setGallery] = useState<GalleryState>(() =>
    readSavedGallery(readGalleryIndexUrl())
  )
  const [loading, setLoading] = useState(false)
  const [loadError, setLoadError] = useState<LoadError | null>(null)
  const [search, setSearch] = useState("")
  const [sourceOpen, setSourceOpen] = useState(false)
  const [sourceDraft, setSourceDraft] = useState(indexUrl)
  const [sourceInvalid, setSourceInvalid] = useState(false)
  const [fileError, setFileError] = useState<FileError | null>(null)
  const [preview, setPreview] = useState<Preview | null>(null)
  const [busyId, setBusyId] = useState<string | null>(null)
  // The single-flight gate lives in a ref: disabling the other cards waits for
  // a re-render, and two clicks in one frame would both see the old null.
  const busyRef = useRef<string | null>(null)
  // Generation guard: a slow refresh that lands after a newer one loses.
  const requestSeq = useRef(0)

  const [installed, setInstalled] = useState<InstalledGalleryPreset[]>(() =>
    readInstalledGalleryPresets()
  )
  const installedRef = useRef(installed)
  useEffect(() => {
    installedRef.current = installed
  }, [installed])
  const installedById = useMemo(
    () => new Map(installed.map((item) => [item.preset.id, item])),
    [installed]
  )

  const updateInstalled = useCallback((next: InstalledGalleryPreset[]) => {
    setInstalled(next)
    writeInstalledGalleryPresets(next)
  }, [])

  // The library is shared between windows through storage, like every other
  // appearance setting.
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === STORAGE_KEY_PRESET_GALLERY_INSTALLED) {
        setInstalled(readInstalledGalleryPresets())
      }
    }
    window.addEventListener("storage", onStorage)
    return () => window.removeEventListener("storage", onStorage)
  }, [])

  // ─── Index ───

  const refresh = useCallback(async (url: string) => {
    const seq = ++requestSeq.current
    setLoading(true)
    setLoadError(null)
    try {
      const result = await refreshGalleryIndex(url)
      if (seq !== requestSeq.current) return
      if (result.ok) {
        setGallery({
          index: result.index,
          skipped: result.skipped,
          fetchedAt: result.fetchedAt,
          fromCache: false,
        })
      } else {
        setLoadError({ kind: "index", issues: result.issues })
      }
    } catch (error) {
      if (seq !== requestSeq.current) return
      setLoadError({ kind: "transport", error })
    } finally {
      if (seq === requestSeq.current) setLoading(false)
    }
  }, [])

  // One load when the section opens, and only when there is no saved copy or
  // it is a day old. No timer and no polling: after this, Refresh is the user's.
  useEffect(() => {
    if (
      gallery.index &&
      gallery.fetchedAt &&
      !isGalleryCacheStale(gallery.fetchedAt)
    ) {
      return
    }
    void refresh(indexUrl)
    // Mount-only by design; later refreshes are explicit.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const onSaveSource = useCallback(() => {
    const next = normalizeGalleryIndexUrl(sourceDraft)
    if (!next) {
      setSourceInvalid(true)
      return
    }
    setSourceInvalid(false)
    writeGalleryIndexUrl(next)
    setIndexUrl(next)
    setSourceDraft(next)
    setGallery(readSavedGallery(next))
    setSourceOpen(false)
    void refresh(next)
  }, [sourceDraft, refresh])

  const onResetSource = useCallback(() => {
    const next = DEFAULT_PRESET_GALLERY_INDEX_URL
    setSourceInvalid(false)
    writeGalleryIndexUrl(next)
    setIndexUrl(next)
    setSourceDraft(next)
    setGallery(readSavedGallery(next))
    setSourceOpen(false)
    void refresh(next)
  }, [refresh])

  // ─── Apply / install / update / remove / preview ───

  const applyWithMode = useCallback(
    (preset: AppearancePreset) => {
      applyPreset(preset)
      if (preset.mode) {
        setTheme(preset.mode)
        syncAppearanceMode(preset.mode)
      }
    },
    [applyPreset, setTheme]
  )

  /** Download through the backend, verified and validated, or a file error. */
  const download = useCallback(async (entry: GalleryEntry) => {
    try {
      const result = await downloadGalleryPreset(entry)
      if (result.ok) return result
      setFileError({
        name: entry.name,
        code: result.code,
        issues: result.code === "invalid-preset" ? result.issues : undefined,
      })
      return null
    } catch (error) {
      setFileError({ name: entry.name, code: "transport", error })
      return null
    }
  }, [])

  const withBusy = useCallback(
    async (id: string, work: () => Promise<void>) => {
      if (busyRef.current !== null) return
      busyRef.current = id
      setBusyId(id)
      try {
        await work()
      } finally {
        busyRef.current = null
        setBusyId(null)
      }
    },
    []
  )

  const onInstall = useCallback(
    (entry: GalleryEntry) =>
      withBusy(entry.id, async () => {
        const result = await download(entry)
        if (!result) return
        updateInstalled(
          upsertInstalledGalleryPreset(
            installedRef.current,
            installedFromDownload(entry, result, indexUrl)
          )
        )
        applyWithMode(result.preset)
        toast.success(t("presetGallery.toasts.installed", { name: entry.name }))
      }),
    [withBusy, download, updateInstalled, indexUrl, applyWithMode, t]
  )

  const onUpdate = useCallback(
    (entry: GalleryEntry) =>
      withBusy(entry.id, async () => {
        const result = await download(entry)
        if (!result) return
        updateInstalled(
          upsertInstalledGalleryPreset(
            installedRef.current,
            installedFromDownload(entry, result, indexUrl)
          )
        )
        // Re-applied only when this preset is the current look, so an update
        // never changes a look the user has since switched away from.
        if (appliedPreset?.id === entry.id) applyWithMode(result.preset)
        toast.success(t("presetGallery.toasts.updated", { name: entry.name }))
      }),
    [
      withBusy,
      download,
      updateInstalled,
      indexUrl,
      appliedPreset?.id,
      applyWithMode,
      t,
    ]
  )

  const onRemove = useCallback(
    (item: InstalledGalleryPreset) => {
      updateInstalled(
        removeInstalledGalleryPreset(installedRef.current, item.preset.id)
      )
      toast.success(
        t("presetGallery.toasts.removed", { name: item.preset.name })
      )
    },
    [updateInstalled, t]
  )

  const onApplyInstalled = useCallback(
    (item: InstalledGalleryPreset) => {
      applyWithMode(item.preset)
      toast.success(t("presets.toasts.applied", { name: item.preset.name }))
    },
    [applyWithMode, t]
  )

  const onPreview = useCallback(
    (entry: GalleryEntry) =>
      withBusy(entry.id, async () => {
        // An installed copy at the listed digest needs no download.
        const local = installedRef.current.find(
          (item) => item.preset.id === entry.id
        )
        const verified =
          local && local.sha256 === entry.sha256
            ? { preset: local.preset, sha256: local.sha256 }
            : await download(entry)
        if (!verified) return
        setPreview({
          entry,
          preset: verified.preset,
          sha256: verified.sha256,
          previousTheme: theme,
        })
        startPresetPreview(verified.preset)
        if (verified.preset.mode) {
          setTheme(verified.preset.mode)
          syncAppearanceMode(verified.preset.mode)
        }
      }),
    [withBusy, download, theme, startPresetPreview, setTheme]
  )

  const onRevertPreview = useCallback(() => {
    if (!preview) return
    endPresetPreview(false)
    if (preview.preset.mode && preview.previousTheme) {
      setTheme(preview.previousTheme)
      syncAppearanceMode(preview.previousTheme)
    }
    setPreview(null)
  }, [preview, endPresetPreview, setTheme])

  const onKeepPreview = useCallback(() => {
    if (!preview) return
    const { entry, preset, sha256 } = preview
    updateInstalled(
      upsertInstalledGalleryPreset(
        installedRef.current,
        installedFromDownload(entry, { preset, sha256 }, indexUrl)
      )
    )
    endPresetPreview(true)
    setPreview(null)
    toast.success(t("presetGallery.toasts.installed", { name: entry.name }))
  }, [preview, updateInstalled, indexUrl, endPresetPreview, t])

  // Leaving the page mid-preview reverts it; a preview is never kept by
  // accident. The latest revert is read through a ref so the cleanup does not
  // re-register on every render.
  const revertRef = useRef(onRevertPreview)
  useEffect(() => {
    revertRef.current = onRevertPreview
  }, [onRevertPreview])
  useEffect(() => () => revertRef.current(), [])

  // ─── Derived ───

  const entries = useMemo(
    () => filterGalleryEntries(gallery.index?.presets ?? [], search),
    [gallery.index, search]
  )
  const listedIds = useMemo(
    () => new Set((gallery.index?.presets ?? []).map((entry) => entry.id)),
    [gallery.index]
  )
  // Installed presets the current index no longer lists still belong to the
  // user; they stay applicable and removable.
  const unlisted = useMemo(
    () =>
      filterGalleryEntries(
        installed
          .filter((item) => !listedIds.has(item.preset.id))
          .map((item) => ({
            id: item.preset.id,
            name: item.preset.name,
            description: item.preset.description,
            author: item.preset.author,
            version: item.preset.version,
            url: item.url,
            sha256: item.sha256,
          })),
        search
      ),
    [installed, listedIds, search]
  )

  const lastRefreshed = useMemo(() => {
    if (!gallery.fetchedAt) return null
    const at = new Date(gallery.fetchedAt)
    if (Number.isNaN(at.getTime())) return null
    return new Intl.DateTimeFormat(locale, {
      dateStyle: "medium",
      timeStyle: "short",
    }).format(at)
  }, [gallery.fetchedAt, locale])

  const tError = t as unknown as AppErrorTranslator
  const describeIndexIssue = (issue: GalleryIssue): string => {
    const message =
      issue.code === "too-large"
        ? t("presetGallery.errors.tooLarge", {
            limit: Math.round(MAX_GALLERY_INDEX_BYTES / 1024),
          })
        : issue.code === "invalid-json"
          ? t("presetGallery.errors.invalidJson")
          : issue.code === "unsupported-version"
            ? t("presetGallery.errors.unsupportedVersion")
            : issue.code === "too-many"
              ? t("presetGallery.errors.tooMany")
              : t("presetGallery.errors.malformed")
    return issue.path ? `${issue.path}: ${message}` : message
  }
  const fileErrorMessage = (error: FileError): string => {
    switch (error.code) {
      case "hash-mismatch":
        return t("presetGallery.errors.hashMismatch")
      case "id-mismatch":
        return t("presetGallery.errors.idMismatch")
      case "invalid-preset":
        return t("presetGallery.errors.invalidPreset")
      case "transport":
        return toLocalizedErrorMessage(error.error, tError)
    }
  }

  const actionsLocked = busyId !== null || preview !== null
  const isDefaultSource = indexUrl === DEFAULT_PRESET_GALLERY_INDEX_URL
  const hasIndex = gallery.index !== null
  const modeLabel = (mode: GalleryEntry["mode"]) =>
    mode ? t(`presets.modes.${mode}`) : null

  const renderCard = (
    entry: GalleryEntry,
    local: InstalledGalleryPreset | undefined,
    listed: boolean
  ) => {
    const status = listed ? galleryEntryStatus(entry, local) : "installed"
    const busy = busyId === entry.id
    const current = appliedPreset?.id === entry.id && presetPreview === null
    const swatches = local
      ? presetSwatches(local.preset, isDarkMode)
      : galleryEntrySwatches(entry, isDarkMode)
    return (
      <div
        key={entry.id}
        role="group"
        aria-label={entry.name}
        className={cn(
          "flex flex-col gap-2 rounded-md border p-3",
          current && "border-primary ring-2 ring-primary/30"
        )}
      >
        <SwatchStrip colors={swatches} />
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="text-xs font-medium">{entry.name}</span>
          {entry.version && (
            <span className="text-2xs text-muted-foreground">
              {entry.version}
            </span>
          )}
          {status === "update" && (
            <Badge className="h-4 px-1 text-2xs">
              {t("presetGallery.updateAvailable")}
            </Badge>
          )}
          {status === "installed" && (
            <Badge variant="secondary" className="h-4 px-1 text-2xs">
              <Check className="size-3" aria-hidden />
              {t("presetGallery.installed")}
            </Badge>
          )}
          {!listed && (
            <Badge variant="outline" className="h-4 px-1 text-2xs">
              {t("presetGallery.notListed")}
            </Badge>
          )}
        </div>
        <span className="text-2xs text-muted-foreground">
          {[
            entry.author
              ? t("presetGallery.byAuthor", { author: entry.author })
              : null,
            modeLabel(entry.mode),
          ]
            .filter(Boolean)
            .join(" · ")}
        </span>
        {entry.description && (
          <span className="line-clamp-2 text-2xs text-muted-foreground leading-4">
            {entry.description}
          </span>
        )}
        <div className="mt-auto flex flex-wrap items-center gap-1.5 pt-1">
          {listed && (
            <Button
              type="button"
              variant="outline"
              size="sm"
              className="h-7 px-2 text-xs"
              disabled={actionsLocked}
              onClick={() => void onPreview(entry)}
            >
              {busy ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                <Eye className="size-3.5" />
              )}
              {t("presetGallery.preview")}
            </Button>
          )}
          {status === "available" ? (
            <Button
              type="button"
              size="sm"
              className="h-7 px-2 text-xs"
              disabled={actionsLocked}
              onClick={() => void onInstall(entry)}
            >
              <Download className="size-3.5" />
              {t("presetGallery.install")}
            </Button>
          ) : (
            <>
              {local && (
                <Button
                  type="button"
                  size="sm"
                  variant={status === "update" ? "outline" : "default"}
                  className="h-7 px-2 text-xs"
                  disabled={actionsLocked}
                  onClick={() => onApplyInstalled(local)}
                >
                  {t("presetGallery.apply")}
                </Button>
              )}
              {status === "update" && (
                <Button
                  type="button"
                  size="sm"
                  className="h-7 px-2 text-xs"
                  disabled={actionsLocked}
                  onClick={() => void onUpdate(entry)}
                >
                  <Download className="size-3.5" />
                  {t("presetGallery.update")}
                </Button>
              )}
              {local && (
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  className="h-7 px-2 text-xs"
                  disabled={actionsLocked}
                  onClick={() => onRemove(local)}
                >
                  <Trash2 className="size-3.5" />
                  {t("presetGallery.remove")}
                </Button>
              )}
            </>
          )}
        </div>
      </div>
    )
  }

  return (
    <section className="rounded-xl border bg-card p-4 space-y-4">
      <div className="flex items-center gap-2">
        <Store className="h-4 w-4 text-muted-foreground" />
        <h2 className="text-sm font-semibold">
          {t("presetGallery.sectionTitle")}
        </h2>
      </div>

      <p className="text-xs text-muted-foreground leading-5">
        {t("presetGallery.sectionDescription")}
      </p>

      {/* ===== Toolbar ===== */}
      <div className="flex flex-wrap items-center gap-2">
        <Input
          value={search}
          onChange={(e) => setSearch(e.target.value)}
          placeholder={t("presetGallery.searchPlaceholder")}
          aria-label={t("presetGallery.searchPlaceholder")}
          className="h-8 w-56"
        />
        <Button
          type="button"
          variant="outline"
          size="sm"
          className="h-8"
          disabled={loading}
          onClick={() => void refresh(indexUrl)}
        >
          {loading ? (
            <Loader2 className="h-4 w-4 animate-spin" />
          ) : (
            <RefreshCw className="h-4 w-4" />
          )}
          {t("presetGallery.refresh")}
        </Button>
        <Button
          type="button"
          variant="ghost"
          size="sm"
          className="h-8"
          aria-expanded={sourceOpen}
          onClick={() => setSourceOpen((open) => !open)}
        >
          <Settings2 className="h-4 w-4" />
          {t("presetGallery.source")}
        </Button>
        <span className="text-2xs text-muted-foreground">
          {lastRefreshed
            ? t("presetGallery.lastRefreshed", { time: lastRefreshed })
            : t("presetGallery.notLoaded")}
        </span>
      </div>

      {sourceOpen && (
        <div className="space-y-2 rounded-md border bg-muted/30 p-3">
          <label
            className="text-xs font-medium text-muted-foreground"
            htmlFor="preset-gallery-source"
          >
            {isDefaultSource
              ? t("presetGallery.sourceDefault")
              : t("presetGallery.sourceCustom")}
          </label>
          <div className="flex flex-wrap items-center gap-2">
            <Input
              id="preset-gallery-source"
              value={sourceDraft}
              onChange={(e) => {
                setSourceDraft(e.target.value)
                setSourceInvalid(false)
              }}
              aria-invalid={sourceInvalid || undefined}
              className="h-8 min-w-64 flex-1 font-mono text-xs"
              spellCheck={false}
            />
            <Button
              type="button"
              size="sm"
              className="h-8"
              disabled={loading}
              onClick={onSaveSource}
            >
              {t("presetGallery.sourceSave")}
            </Button>
            {!isDefaultSource && (
              <Button
                type="button"
                variant="ghost"
                size="sm"
                className="h-8"
                disabled={loading}
                onClick={onResetSource}
              >
                {t("presetGallery.sourceReset")}
              </Button>
            )}
          </div>
          <p
            className={cn(
              "text-2xs leading-4",
              sourceInvalid ? "text-destructive" : "text-muted-foreground"
            )}
          >
            {sourceInvalid
              ? t("presetGallery.sourceInvalid")
              : t("presetGallery.sourceHint")}
          </p>
        </div>
      )}

      {/* ===== Preview bar ===== */}
      {preview && (
        <div
          role="status"
          className="flex flex-wrap items-center gap-2 rounded-md border border-primary/40 bg-primary/5 p-3"
        >
          <Eye className="size-4 text-primary" aria-hidden />
          <div className="min-w-0 flex-1">
            <p className="text-xs font-medium">
              {t("presetGallery.previewing", { name: preview.entry.name })}
            </p>
            <p className="text-2xs text-muted-foreground leading-4">
              {t("presetGallery.previewHint")}
            </p>
          </div>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            className="h-7 px-2 text-xs"
            onClick={onRevertPreview}
          >
            {t("presetGallery.revert")}
          </Button>
          <Button
            type="button"
            size="sm"
            className="h-7 px-2 text-xs"
            onClick={onKeepPreview}
          >
            <Check className="size-3.5" />
            {t("presetGallery.keep")}
          </Button>
        </div>
      )}

      {/* ===== Load error (the saved copy, if any, stays on screen) ===== */}
      {loadError && (
        <div
          role="alert"
          className="space-y-1 rounded-md border border-destructive/40 p-3"
        >
          <p className="text-xs text-destructive">
            {loadError.kind === "index"
              ? t("presetGallery.indexInvalid")
              : t("presetGallery.loadFailed")}
          </p>
          {loadError.kind === "index" ? (
            <ul className="max-h-32 space-y-0.5 overflow-auto font-mono text-2xs text-muted-foreground">
              {loadError.issues.map((issue, index) => (
                <li key={`${issue.path}-${index}`}>
                  {describeIndexIssue(issue)}
                </li>
              ))}
            </ul>
          ) : (
            <p className="text-2xs text-muted-foreground">
              {toLocalizedErrorMessage(loadError.error, tError)}
            </p>
          )}
          {hasIndex && (
            <p className="text-2xs text-muted-foreground">
              {t("presetGallery.savedCopy")}
            </p>
          )}
          <Button
            type="button"
            variant="outline"
            size="sm"
            className="h-7 px-2 text-xs"
            disabled={loading}
            onClick={() => void refresh(indexUrl)}
          >
            <RefreshCw className="size-3.5" />
            {t("presetGallery.retry")}
          </Button>
        </div>
      )}

      {gallery.skipped.length > 0 && (
        <p
          className="text-2xs text-muted-foreground"
          title={gallery.skipped
            .map((issue) => `${issue.path}: ${issue.code}`)
            .join("\n")}
        >
          {t("presetGallery.skipped", { count: gallery.skipped.length })}
        </p>
      )}

      {/* ===== Cards ===== */}
      {!hasIndex && loading ? (
        <div className="flex items-center justify-center py-8">
          <Loader2 className="h-5 w-5 animate-spin text-muted-foreground/50" />
        </div>
      ) : hasIndex && entries.length === 0 && unlisted.length === 0 ? (
        <p className="py-6 text-center text-xs text-muted-foreground">
          {gallery.index?.presets.length === 0
            ? t("presetGallery.emptyIndex")
            : t("presetGallery.empty")}
        </p>
      ) : (
        <div
          className={cn(
            "grid grid-cols-1 gap-2 transition-opacity sm:grid-cols-2 lg:grid-cols-3",
            loading && "opacity-70"
          )}
          role="group"
          aria-label={t("presetGallery.sectionTitle")}
        >
          {entries.map((entry) =>
            renderCard(entry, installedById.get(entry.id), true)
          )}
          {unlisted.map((entry) =>
            renderCard(entry, installedById.get(entry.id), false)
          )}
        </div>
      )}

      {/* ===== File error ===== */}
      <Dialog
        open={fileError !== null}
        onOpenChange={(open) => {
          if (!open) setFileError(null)
        }}
      >
        <DialogContent className="sm:max-w-lg">
          <DialogHeader>
            <DialogTitle>{t("presetGallery.fileErrorTitle")}</DialogTitle>
            <DialogDescription>
              {t("presetGallery.fileErrorDescription")}
            </DialogDescription>
          </DialogHeader>
          {fileError && (
            <div className="space-y-2 text-xs">
              <p className="text-sm font-medium">{fileError.name}</p>
              <p className="text-muted-foreground leading-5">
                {fileErrorMessage(fileError)}
              </p>
              {fileError.issues && fileError.issues.length > 0 && (
                <ul
                  className="max-h-64 space-y-1 overflow-auto font-mono text-2xs"
                  aria-label={t("presets.importErrorsTitle")}
                >
                  {fileError.issues.map((issue, index) => (
                    <li key={`${issue.path}-${index}`}>
                      {issue.path ? `${issue.path}: ` : ""}
                      {t(`presets.errors.${issue.code}`, {
                        limit: Math.round(MAX_PRESET_BYTES / 1024),
                      })}
                      {issue.detail ? ` (${issue.detail})` : ""}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          )}
          <DialogFooter>
            <Button variant="ghost" onClick={() => setFileError(null)}>
              {t("presets.cancel")}
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </section>
  )
}
