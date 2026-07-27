"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  AlertCircle,
  Check,
  ImagePlus,
  Loader2,
  Package,
  Search,
  X,
} from "lucide-react"
import { toast } from "sonner"

import {
  acpAddRegistryAgent,
  acpFetchRegistryCatalog,
  acpSaveCustomAgent,
  type CustomAgentSpec,
  type CustomDistributionKind,
  type RegistryCatalogAgent,
} from "@/lib/api"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Checkbox } from "@/components/ui/checkbox"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Label } from "@/components/ui/label"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { Textarea } from "@/components/ui/textarea"
import { cn } from "@/lib/utils"

interface AddCustomAgentDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** Called after a successful add so the caller can refresh its agent list. */
  onAdded: () => void
}

/**
 * Validate + normalize the manual form's JSON field.
 *
 * Accepts either a bare `distribution` object (`{"npx": {...}}`) or a whole
 * registry agent entry (`{"id": …, "distribution": {...}}`) — users copy both
 * shapes out of the registry, and rejecting one of them would be a needless
 * papercut.
 */
export function parseManualSpec(raw: string): {
  spec?: CustomAgentSpec
  registryId?: string
  name?: string
  description?: string
  version?: string
  iconUrl?: string
  error?: string
} {
  const trimmed = raw.trim()
  if (!trimmed) return { error: "empty" }
  let parsed: unknown
  try {
    parsed = JSON.parse(trimmed)
  } catch {
    return { error: "invalidJson" }
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return { error: "invalidJson" }
  }
  const obj = parsed as Record<string, unknown>
  // A whole registry entry: unwrap it and keep the metadata it carries.
  if (obj.distribution && typeof obj.distribution === "object") {
    return {
      spec: obj.distribution as CustomAgentSpec,
      registryId: typeof obj.id === "string" ? obj.id : undefined,
      name: typeof obj.name === "string" ? obj.name : undefined,
      description:
        typeof obj.description === "string" ? obj.description : undefined,
      version: typeof obj.version === "string" ? obj.version : undefined,
      iconUrl: typeof obj.icon === "string" ? obj.icon : undefined,
    }
  }
  if (!obj.npx && !obj.uvx && !obj.binary) {
    return { error: "noDistribution" }
  }
  return { spec: obj as CustomAgentSpec }
}

/**
 * Largest icon file the manual form accepts, mirroring the backend's
 * `MAX_ICON_BYTES`. Registry marks are 650 B–5 KB SVGs, so this is generous;
 * the point is that the icon is stored inline in the agent row.
 */
export const MAX_ICON_BYTES = 256 * 1024

/** Which channels a pasted spec publishes, binary first. */
export function specKinds(spec: CustomAgentSpec): CustomDistributionKind[] {
  const kinds: CustomDistributionKind[] = []
  if (spec.binary && Object.keys(spec.binary).length > 0) kinds.push("binary")
  if (spec.npx) kinds.push("npx")
  if (spec.uvx) kinds.push("uvx")
  return kinds
}

/**
 * An entry's mark in the picker. The URL points at the ACP CDN and is fetched
 * by the webview directly — this list is inherently online (it just downloaded
 * the registry), and an unreachable icon degrades to the generic package glyph
 * rather than a broken image. Only once the agent is added does the backend
 * inline the icon, so the settings list works offline afterwards.
 */
function RegistryEntryIcon({ iconUrl }: { iconUrl: string | null }) {
  const [failed, setFailed] = useState(false)
  if (!iconUrl || failed) {
    return <Package className="h-4 w-4 mt-0.5 shrink-0 text-muted-foreground" />
  }
  return (
    // eslint-disable-next-line @next/next/no-img-element
    <img
      src={iconUrl}
      alt=""
      aria-hidden
      onError={() => setFailed(true)}
      className="h-4 w-4 mt-0.5 shrink-0 rounded-[3px] object-contain"
    />
  )
}

export function AddCustomAgentDialog({
  open,
  onOpenChange,
  onAdded,
}: AddCustomAgentDialogProps) {
  const t = useTranslations("AcpAgentSettings")

  const [catalog, setCatalog] = useState<RegistryCatalogAgent[]>([])
  const [loadingCatalog, setLoadingCatalog] = useState(false)
  const [catalogError, setCatalogError] = useState<string | null>(null)
  const [query, setQuery] = useState("")
  const [addingId, setAddingId] = useState<string | null>(null)

  const [manualId, setManualId] = useState("")
  const [manualName, setManualName] = useState("")
  const [manualVersion, setManualVersion] = useState("")
  const [manualJson, setManualJson] = useState("")
  const [manualKind, setManualKind] = useState<CustomDistributionKind | null>(
    null
  )
  const [manualIcon, setManualIcon] = useState<string | null>(null)
  const [manualSkills, setManualSkills] = useState(false)
  const [savingManual, setSavingManual] = useState(false)
  const iconInputRef = useRef<HTMLInputElement>(null)

  const loadCatalog = useCallback(async () => {
    setLoadingCatalog(true)
    setCatalogError(null)
    try {
      setCatalog(await acpFetchRegistryCatalog())
    } catch (err) {
      setCatalogError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoadingCatalog(false)
    }
  }, [])

  useEffect(() => {
    if (!open) return
    void loadCatalog()
  }, [open, loadCatalog])

  // Reset the form each time the dialog opens so a previous attempt never
  // leaks into the next one.
  useEffect(() => {
    if (open) return
    setQuery("")
    setManualId("")
    setManualName("")
    setManualVersion("")
    setManualJson("")
    setManualKind(null)
    setManualIcon(null)
    setManualSkills(false)
  }, [open])

  const handlePickIcon = useCallback(
    (file: File | undefined) => {
      // Reset the input so re-picking the same file fires `change` again.
      if (iconInputRef.current) iconInputRef.current.value = ""
      if (!file) return
      if (!file.type.startsWith("image/")) {
        toast.error(t("customAgentIconNotAnImage"))
        return
      }
      if (file.size > MAX_ICON_BYTES) {
        toast.error(
          t("customAgentIconTooLarge", {
            limit: Math.round(MAX_ICON_BYTES / 1024),
          })
        )
        return
      }
      // The icon is stored inline with the definition, so it must be read into
      // a data URL here — a path would not survive the DB round-trip, and a
      // blob URL would not survive the reload.
      const reader = new FileReader()
      reader.onload = () => {
        const result = reader.result
        if (typeof result === "string") setManualIcon(result)
      }
      reader.onerror = () => toast.error(t("customAgentIconReadFailed"))
      reader.readAsDataURL(file)
    },
    [t]
  )

  const addable = useMemo(() => {
    const needle = query.trim().toLowerCase()
    return catalog
      .filter((entry) => !entry.builtin)
      .filter(
        (entry) =>
          !needle ||
          entry.name.toLowerCase().includes(needle) ||
          entry.registryId.toLowerCase().includes(needle) ||
          entry.description.toLowerCase().includes(needle)
      )
  }, [catalog, query])

  const handleAddFromRegistry = useCallback(
    async (entry: RegistryCatalogAgent) => {
      setAddingId(entry.registryId)
      try {
        await acpAddRegistryAgent(entry.registryId)
        toast.success(t("customAgentAdded", { name: entry.name }))
        onAdded()
        onOpenChange(false)
      } catch (err) {
        toast.error(err instanceof Error ? err.message : String(err))
      } finally {
        setAddingId(null)
      }
    },
    [onAdded, onOpenChange, t]
  )

  const manualParsed = useMemo(
    () => (manualJson.trim() ? parseManualSpec(manualJson) : null),
    [manualJson]
  )
  const manualKinds = manualParsed?.spec ? specKinds(manualParsed.spec) : []
  const effectiveKind = manualKind ?? manualKinds[0] ?? null
  const effectiveId = manualId.trim() || manualParsed?.registryId?.trim() || ""
  const effectiveName = manualName.trim() || manualParsed?.name?.trim() || ""
  const manualReady =
    Boolean(manualParsed?.spec) &&
    Boolean(effectiveId) &&
    Boolean(effectiveKind)

  const handleSaveManual = useCallback(async () => {
    if (!manualParsed?.spec || !effectiveKind || !effectiveId) return
    setSavingManual(true)
    try {
      await acpSaveCustomAgent({
        registryId: effectiveId,
        name: effectiveName || effectiveId,
        description: manualParsed.description ?? "",
        version: manualVersion.trim() || manualParsed.version || "",
        distributionKind: effectiveKind,
        spec: manualParsed.spec,
        // An upload wins over an `icon` URL carried by a pasted registry entry.
        iconUrl: manualIcon ?? manualParsed.iconUrl ?? null,
        skillsSharedStore: manualSkills,
      })
      toast.success(
        t("customAgentAdded", { name: effectiveName || effectiveId })
      )
      onAdded()
      onOpenChange(false)
    } catch (err) {
      toast.error(err instanceof Error ? err.message : String(err))
    } finally {
      setSavingManual(false)
    }
  }, [
    manualParsed,
    effectiveKind,
    effectiveId,
    effectiveName,
    manualVersion,
    manualIcon,
    manualSkills,
    onAdded,
    onOpenChange,
    t,
  ])

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-2xl">
        <DialogHeader>
          <DialogTitle>{t("addCustomAgent")}</DialogTitle>
          <DialogDescription>{t("addCustomAgentHint")}</DialogDescription>
        </DialogHeader>

        <Tabs defaultValue="registry" className="min-h-0">
          <TabsList className="w-full">
            <TabsTrigger value="registry" className="flex-1">
              {t("customAgentFromRegistry")}
            </TabsTrigger>
            <TabsTrigger value="manual" className="flex-1">
              {t("customAgentManual")}
            </TabsTrigger>
          </TabsList>

          <TabsContent value="registry" className="mt-3 space-y-3">
            <div className="relative">
              <Search className="absolute left-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground" />
              <Input
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                placeholder={t("customAgentSearchPlaceholder")}
                className="pl-7 h-8 text-xs"
              />
            </div>

            {loadingCatalog && (
              <div className="flex items-center justify-center gap-2 py-8 text-xs text-muted-foreground">
                <Loader2 className="h-3.5 w-3.5 animate-spin" />
                {t("customAgentLoadingCatalog")}
              </div>
            )}

            {catalogError && !loadingCatalog && (
              <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
                <div className="flex items-center gap-2">
                  <AlertCircle className="h-3.5 w-3.5 shrink-0" />
                  <span className="min-w-0 break-all">{catalogError}</span>
                </div>
                <Button
                  variant="ghost"
                  size="sm"
                  className="mt-1 h-6 text-xs"
                  onClick={() => void loadCatalog()}
                >
                  {t("customAgentRetry")}
                </Button>
              </div>
            )}

            {!loadingCatalog && !catalogError && (
              <div className="max-h-[46vh] overflow-y-auto space-y-1.5 pr-1">
                {addable.length === 0 && (
                  <div className="py-8 text-center text-xs text-muted-foreground">
                    {t("customAgentNoResults")}
                  </div>
                )}
                {addable.map((entry) => {
                  const disabled =
                    entry.installed ||
                    !entry.supportedOnPlatform ||
                    entry.distributionKinds.length === 0
                  return (
                    <div
                      key={entry.registryId}
                      className={cn(
                        "rounded-md border px-3 py-2 flex items-start gap-3",
                        disabled && "opacity-60"
                      )}
                    >
                      <RegistryEntryIcon iconUrl={entry.iconUrl} />
                      <div className="min-w-0 flex-1">
                        <div className="flex items-center gap-2 flex-wrap">
                          <span className="text-sm font-medium truncate">
                            {entry.name}
                          </span>
                          {entry.version && (
                            <Badge variant="outline" className="text-[10px]">
                              {entry.version}
                            </Badge>
                          )}
                          {entry.distributionKinds.map((kind) => (
                            <Badge
                              key={kind}
                              variant="secondary"
                              className="text-[10px]"
                            >
                              {kind}
                            </Badge>
                          ))}
                        </div>
                        <p className="text-xs text-muted-foreground mt-0.5 line-clamp-2">
                          {entry.description}
                        </p>
                        {!entry.supportedOnPlatform && (
                          <p className="text-[11px] text-amber-500 mt-1">
                            {t("customAgentUnsupportedPlatform")}
                          </p>
                        )}
                      </div>
                      <Button
                        size="sm"
                        variant={entry.installed ? "ghost" : "secondary"}
                        className="h-7 text-xs shrink-0"
                        disabled={disabled || addingId !== null}
                        onClick={() => void handleAddFromRegistry(entry)}
                      >
                        {addingId === entry.registryId ? (
                          <Loader2 className="h-3.5 w-3.5 animate-spin" />
                        ) : entry.installed ? (
                          <>
                            <Check className="h-3.5 w-3.5 mr-1" />
                            {t("customAgentAlreadyAdded")}
                          </>
                        ) : (
                          t("customAgentAdd")
                        )}
                      </Button>
                    </div>
                  )
                })}
              </div>
            )}
          </TabsContent>

          <TabsContent value="manual" className="mt-3 space-y-3">
            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <Label className="text-xs">{t("customAgentIdLabel")}</Label>
                <Input
                  value={effectiveId}
                  onChange={(e) => setManualId(e.target.value)}
                  placeholder="goose"
                  className="h-8 text-xs font-mono"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("customAgentNameLabel")}</Label>
                <Input
                  value={effectiveName}
                  onChange={(e) => setManualName(e.target.value)}
                  placeholder="Goose"
                  className="h-8 text-xs"
                />
              </div>
            </div>

            <div className="grid grid-cols-2 gap-3">
              <div className="space-y-1">
                <Label className="text-xs">
                  {t("customAgentVersionLabel")}
                </Label>
                <Input
                  value={manualVersion || manualParsed?.version || ""}
                  onChange={(e) => setManualVersion(e.target.value)}
                  placeholder="1.44.0"
                  className="h-8 text-xs font-mono"
                />
              </div>
              <div className="space-y-1">
                <Label className="text-xs">{t("customAgentIconLabel")}</Label>
                <div className="flex items-center gap-2">
                  {manualIcon ? (
                    <span className="relative inline-flex h-8 w-8 shrink-0 items-center justify-center rounded border bg-muted/30">
                      {/* eslint-disable-next-line @next/next/no-img-element */}
                      <img
                        src={manualIcon}
                        alt=""
                        className="h-5 w-5 object-contain"
                      />
                      <button
                        type="button"
                        aria-label={t("customAgentIconClear")}
                        title={t("customAgentIconClear")}
                        className="absolute -right-1.5 -top-1.5 inline-flex h-4 w-4 items-center justify-center rounded-full border bg-background text-muted-foreground hover:text-foreground"
                        onClick={() => setManualIcon(null)}
                      >
                        <X className="h-2.5 w-2.5" />
                      </button>
                    </span>
                  ) : null}
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    className="h-8 text-xs"
                    onClick={() => iconInputRef.current?.click()}
                  >
                    <ImagePlus className="h-3.5 w-3.5" />
                    {manualIcon
                      ? t("customAgentIconReplace")
                      : t("customAgentIconUpload")}
                  </Button>
                  <input
                    ref={iconInputRef}
                    type="file"
                    accept="image/*"
                    className="hidden"
                    onChange={(e) => handlePickIcon(e.target.files?.[0])}
                  />
                </div>
                <p className="text-[11px] text-muted-foreground">
                  {t("customAgentIconHint")}
                </p>
              </div>
            </div>

            <div className="space-y-1">
              <Label className="text-xs">{t("customAgentSpecLabel")}</Label>
              <Textarea
                value={manualJson}
                onChange={(e) => setManualJson(e.target.value)}
                rows={9}
                spellCheck={false}
                placeholder={
                  '{\n  "npx": {\n    "package": "some-acp-agent@1.0.0",\n    "args": ["--acp"]\n  }\n}'
                }
                className="text-xs font-mono"
              />
              <p className="text-[11px] text-muted-foreground">
                {t("customAgentSpecHint")}
              </p>
            </div>

            {manualParsed?.error && (
              <div className="rounded-md border border-red-500/30 bg-red-500/5 px-3 py-2 text-xs text-red-400">
                {manualParsed.error === "invalidJson"
                  ? t("customAgentInvalidJson")
                  : t("customAgentNoDistribution")}
              </div>
            )}

            {manualKinds.length > 1 && (
              <div className="space-y-1">
                <Label className="text-xs">{t("customAgentKindLabel")}</Label>
                <div className="flex gap-1.5">
                  {manualKinds.map((kind) => (
                    <Button
                      key={kind}
                      type="button"
                      size="sm"
                      variant={effectiveKind === kind ? "secondary" : "outline"}
                      className="h-7 text-xs"
                      onClick={() => setManualKind(kind)}
                    >
                      {kind}
                    </Button>
                  ))}
                </div>
              </div>
            )}

            <div className="flex items-start gap-2">
              <Checkbox
                id="custom-agent-skills"
                checked={manualSkills}
                onCheckedChange={(v) => setManualSkills(v === true)}
                className="mt-0.5"
              />
              <div className="space-y-0.5">
                <Label htmlFor="custom-agent-skills" className="text-xs">
                  {t("customAgentSkillsLabel")}
                </Label>
                <p className="text-[11px] text-muted-foreground">
                  {t("customAgentSkillsHint")}
                </p>
              </div>
            </div>
          </TabsContent>
        </Tabs>

        <DialogFooter>
          <Button variant="ghost" onClick={() => onOpenChange(false)}>
            {t("customAgentCancel")}
          </Button>
          <Button
            onClick={() => void handleSaveManual()}
            disabled={!manualReady || savingManual}
          >
            {savingManual && (
              <Loader2 className="h-3.5 w-3.5 mr-1.5 animate-spin" />
            )}
            {t("customAgentSave")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
