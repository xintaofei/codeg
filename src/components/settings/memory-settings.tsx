"use client"

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import {
  Brain,
  Database,
  Edit2,
  HardDrive,
  Loader2,
  Plus,
  Search,
  Trash2,
} from "lucide-react"
import { toast } from "sonner"

import {
  SettingCard,
  SettingNote,
  SettingRow,
} from "@/components/shared/setting-card"
import {
  SettingsError,
  SettingsSaveBar,
  SettingsSection,
} from "@/components/shared/settings-section"
import { Badge } from "@/components/ui/badge"
import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { RadioGroup, RadioGroupItem } from "@/components/ui/radio-group"
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import {
  memoryKindCreate,
  memoryKindDelete,
  memoryKindList,
  memoryKindUpdate,
  memoryNodeDelete,
  memorySearch,
  memorySettingsGet,
  memorySettingsSet,
  subscribeMemoryChanged,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import type {
  ExternalMcpMapping,
  MemoryBackendKind,
  MemoryHit,
  MemoryKind,
  MemoryKindDraft,
  MemoryMode,
  MemoryScope,
  MemorySettings as MemorySettingsType,
} from "@/lib/types"
import { MemoryKindDialog } from "./memory-kind-dialog"

export function MemorySettings() {
  const t = useTranslations("Memory")

  const [loading, setLoading] = useState(true)
  const [savingSettings, setSavingSettings] = useState(false)
  const [loadError, setLoadError] = useState<string | null>(null)

  // Settings state
  const [backend, setBackend] = useState<MemoryBackendKind>("off")
  const [scope, setScope] = useState<MemoryScope>("project")
  const [externalMapping, setExternalMapping] = useState<ExternalMcpMapping>({
    server_id: "",
    write_tool: "memory_write",
    search_tool: "memory_search",
    link_tool: "memory_link",
  })

  // Kinds state
  const [kinds, setKinds] = useState<MemoryKind[]>([])
  const [kindDialogOpen, setKindDialogOpen] = useState(false)
  const [editingKind, setEditingKind] = useState<MemoryKind | null>(null)
  const [savingKind, setSavingKind] = useState(false)

  // Search state
  const [searchQuery, setSearchQuery] = useState("")
  const [searching, setSearching] = useState(false)
  const [searchHits, setSearchHits] = useState<MemoryHit[] | null>(null)

  const loadData = useCallback(async () => {
    try {
      setLoading(true)
      setLoadError(null)
      const [settingsRes, kindsRes] = await Promise.all([
        memorySettingsGet(),
        memoryKindList(),
      ])

      setBackend(settingsRes.backend)
      setScope(settingsRes.scope)
      if (settingsRes.external) {
        setExternalMapping(settingsRes.external)
      }
      setKinds(kindsRes)
    } catch (err) {
      setLoadError(toErrorMessage(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadData()

    const unsubscribe = subscribeMemoryChanged((change) => {
      if (change.kind === "settings") {
        void memorySettingsGet().then((s) => {
          setBackend(s.backend)
          setScope(s.scope)
          if (s.external) setExternalMapping(s.external)
        })
      } else if (change.kind === "kinds") {
        void memoryKindList().then(setKinds)
      }
    })

    return () => {
      void unsubscribe.then((unsub) => unsub?.())
    }
  }, [loadData])

  const handleSaveSettings = async () => {
    try {
      setSavingSettings(true)
      const nextSettings: MemorySettingsType = {
        backend,
        scope,
        external: backend === "external_mcp" ? externalMapping : null,
      }
      await memorySettingsSet(nextSettings)
      toast.success(t("saveSuccess"))
    } catch (err) {
      toast.error(toErrorMessage(err) || t("saveError"))
    } finally {
      setSavingSettings(false)
    }
  }

  const handleKindModeChange = async (
    kind: MemoryKind,
    newMode: MemoryMode
  ) => {
    try {
      const updated = await memoryKindUpdate(kind.id, {
        name: kind.name,
        instruction: kind.instruction,
        mode: newMode,
      })
      setKinds((prev) => prev.map((k) => (k.id === kind.id ? updated : k)))
      toast.success(t("updateSuccess"))
    } catch (err) {
      toast.error(toErrorMessage(err) || t("updateError"))
    }
  }

  const handleKindSave = async (draft: MemoryKindDraft) => {
    try {
      setSavingKind(true)
      if (editingKind) {
        const updated = await memoryKindUpdate(editingKind.id, draft)
        setKinds((prev) =>
          prev.map((k) => (k.id === editingKind.id ? updated : k))
        )
        toast.success(t("updateSuccess"))
      } else {
        const created = await memoryKindCreate(draft)
        setKinds((prev) => [...prev, created])
        toast.success(t("createSuccess"))
      }
      setKindDialogOpen(false)
      setEditingKind(null)
    } catch (err) {
      toast.error(
        toErrorMessage(err) ||
          (editingKind ? t("updateError") : t("createError"))
      )
    } finally {
      setSavingKind(false)
    }
  }

  const handleKindDelete = async (kind: MemoryKind) => {
    if (kind.builtin) return
    try {
      await memoryKindDelete(kind.id)
      setKinds((prev) => prev.filter((k) => k.id !== kind.id))
      toast.success(t("deleteSuccess"))
    } catch (err) {
      toast.error(toErrorMessage(err) || t("deleteError"))
    }
  }

  const handleSearch = async (e?: React.FormEvent) => {
    if (e) e.preventDefault()
    const q = searchQuery.trim()
    if (!q) {
      setSearchHits(null)
      return
    }

    try {
      setSearching(true)
      const results = await memorySearch(q)
      setSearchHits(results)
    } catch (err) {
      toast.error(toErrorMessage(err))
    } finally {
      setSearching(false)
    }
  }

  const handleForgetNode = async (nodeId: number) => {
    try {
      await memoryNodeDelete(nodeId)
      setSearchHits((prev) =>
        prev ? prev.filter((hit) => hit.node.id !== nodeId) : null
      )
      toast.success(t("nodeDeleteSuccess"))
    } catch (err) {
      toast.error(toErrorMessage(err) || t("nodeDeleteError"))
    }
  }

  if (loading) {
    return (
      <div className="flex h-full items-center justify-center p-8">
        <Loader2 className="size-6 animate-spin text-muted-foreground" />
      </div>
    )
  }

  return (
    <ScrollArea className="h-full">
      <div className="w-full space-y-4 p-3 md:p-4">
        {loadError && <SettingsError>{loadError}</SettingsError>}

        {/* ─── Storage Backend ─── */}
        <SettingsSection
          icon={Database}
          title={t("backend")}
          description={t("backendHint")}
        >
          <RadioGroup
            value={backend}
            onValueChange={(val) => setBackend(val as MemoryBackendKind)}
            className="grid gap-2"
          >
            <label
              htmlFor="backend-off"
              className="flex items-start gap-3 rounded-xl border border-border/70 p-3 transition-colors hover:bg-muted/50 cursor-pointer has-data-[state=checked]:border-primary has-data-[state=checked]:bg-primary/5"
            >
              <RadioGroupItem value="off" id="backend-off" className="mt-0.5" />
              <div className="min-w-0 flex-1 space-y-0.5">
                <div className="text-sm font-medium">{t("backendOff")}</div>
              </div>
            </label>

            <label
              htmlFor="backend-local"
              className="flex items-start gap-3 rounded-xl border border-border/70 p-3 transition-colors hover:bg-muted/50 cursor-pointer has-data-[state=checked]:border-primary has-data-[state=checked]:bg-primary/5"
            >
              <RadioGroupItem
                value="local_sqlite"
                id="backend-local"
                className="mt-0.5"
              />
              <div className="min-w-0 flex-1 space-y-0.5">
                <div className="text-sm font-medium">{t("backendLocal")}</div>
              </div>
            </label>

            <label
              htmlFor="backend-external"
              className="flex items-start gap-3 rounded-xl border border-border/70 p-3 transition-colors hover:bg-muted/50 cursor-pointer has-data-[state=checked]:border-primary has-data-[state=checked]:bg-primary/5"
            >
              <RadioGroupItem
                value="external_mcp"
                id="backend-external"
                className="mt-0.5"
              />
              <div className="min-w-0 flex-1 space-y-0.5">
                <div className="text-sm font-medium">
                  {t("backendExternal")}
                </div>
              </div>
            </label>
          </RadioGroup>

          {backend === "external_mcp" && (
            <SettingCard className="mt-3">
              <SettingRow title={t("externalServer")} htmlFor="external-server">
                <Input
                  id="external-server"
                  value={externalMapping.server_id}
                  onChange={(e) =>
                    setExternalMapping((prev) => ({
                      ...prev,
                      server_id: e.target.value,
                    }))
                  }
                  placeholder="e.g. memory-server"
                />
              </SettingRow>

              <SettingRow
                title={t("externalWriteTool")}
                htmlFor="external-write-tool"
              >
                <Input
                  id="external-write-tool"
                  value={externalMapping.write_tool}
                  onChange={(e) =>
                    setExternalMapping((prev) => ({
                      ...prev,
                      write_tool: e.target.value,
                    }))
                  }
                  placeholder="memory_write"
                />
              </SettingRow>

              <SettingRow
                title={t("externalSearchTool")}
                htmlFor="external-search-tool"
              >
                <Input
                  id="external-search-tool"
                  value={externalMapping.search_tool}
                  onChange={(e) =>
                    setExternalMapping((prev) => ({
                      ...prev,
                      search_tool: e.target.value,
                    }))
                  }
                  placeholder="memory_search"
                />
              </SettingRow>

              <SettingRow
                title={t("externalLinkTool")}
                htmlFor="external-link-tool"
              >
                <Input
                  id="external-link-tool"
                  value={externalMapping.link_tool}
                  onChange={(e) =>
                    setExternalMapping((prev) => ({
                      ...prev,
                      link_tool: e.target.value,
                    }))
                  }
                  placeholder="memory_link"
                />
              </SettingRow>
            </SettingCard>
          )}
        </SettingsSection>

        {/* ─── Scope ─── */}
        <SettingsSection icon={HardDrive} title={t("scope")}>
          <RadioGroup
            value={scope}
            onValueChange={(val) => setScope(val as MemoryScope)}
            className="grid gap-2 sm:grid-cols-2"
          >
            <label
              htmlFor="scope-project"
              className="flex items-start gap-3 rounded-xl border border-border/70 p-3 transition-colors hover:bg-muted/50 cursor-pointer has-data-[state=checked]:border-primary has-data-[state=checked]:bg-primary/5"
            >
              <RadioGroupItem
                value="project"
                id="scope-project"
                className="mt-0.5"
              />
              <div className="min-w-0 flex-1 space-y-0.5">
                <div className="text-sm font-medium">{t("scopeProject")}</div>
              </div>
            </label>

            <label
              htmlFor="scope-global"
              className="flex items-start gap-3 rounded-xl border border-border/70 p-3 transition-colors hover:bg-muted/50 cursor-pointer has-data-[state=checked]:border-primary has-data-[state=checked]:bg-primary/5"
            >
              <RadioGroupItem
                value="global"
                id="scope-global"
                className="mt-0.5"
              />
              <div className="min-w-0 flex-1 space-y-0.5">
                <div className="text-sm font-medium">{t("scopeGlobal")}</div>
              </div>
            </label>
          </RadioGroup>

          <SettingsSaveBar
            onSave={handleSaveSettings}
            saving={savingSettings}
            label={t("saveSettings")}
            savingLabel={t("saveSettings")}
          />
        </SettingsSection>

        {/* ─── Memory Kinds ─── */}
        <SettingsSection
          icon={Brain}
          title={t("kinds")}
          description={t("kindsHint")}
          control={
            <Button
              size="sm"
              onClick={() => {
                setEditingKind(null)
                setKindDialogOpen(true)
              }}
              disabled={backend === "off"}
            >
              <Plus className="size-3.5 mr-1" />
              {t("addKind")}
            </Button>
          }
        >
          {backend === "off" ? (
            <SettingNote>{t("disabledHint")}</SettingNote>
          ) : (
            <SettingCard>
              {kinds.map((kind) => (
                <div
                  key={kind.id}
                  className="flex flex-col gap-2 p-3 sm:flex-row sm:items-center sm:justify-between"
                >
                  <div className="min-w-0 flex-1 space-y-1">
                    <div className="flex items-center gap-2">
                      <span className="text-sm font-medium">{kind.name}</span>
                      {kind.builtin && (
                        <Badge variant="secondary" className="text-3xs">
                          {t("builtin")}
                        </Badge>
                      )}
                    </div>
                    <p className="text-xs text-muted-foreground line-clamp-2">
                      {kind.instruction}
                    </p>
                  </div>

                  <div className="flex items-center gap-2 shrink-0">
                    <Select
                      value={kind.mode}
                      onValueChange={(val) =>
                        handleKindModeChange(kind, val as MemoryMode)
                      }
                    >
                      <SelectTrigger
                        className="w-32 h-8 text-xs"
                        aria-label={`${kind.name} mode`}
                      >
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="auto">{t("modeAuto")}</SelectItem>
                        <SelectItem value="on_request">
                          {t("modeOnRequest")}
                        </SelectItem>
                        <SelectItem value="off">{t("modeOff")}</SelectItem>
                      </SelectContent>
                    </Select>

                    {!kind.builtin && (
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 text-muted-foreground hover:text-foreground"
                        onClick={() => {
                          setEditingKind(kind)
                          setKindDialogOpen(true)
                        }}
                        aria-label={`Edit ${kind.name}`}
                      >
                        <Edit2 className="size-3.5" />
                      </Button>
                    )}

                    {!kind.builtin ? (
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 text-muted-foreground hover:text-destructive"
                        onClick={() => handleKindDelete(kind)}
                        aria-label={`Delete ${kind.name}`}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    ) : (
                      <Button
                        variant="ghost"
                        size="icon"
                        className="h-8 w-8 text-muted-foreground opacity-30 cursor-not-allowed"
                        disabled
                        title={t("deleteKindBuiltin")}
                        aria-label={`Delete ${kind.name} (Built-in)`}
                      >
                        <Trash2 className="size-3.5" />
                      </Button>
                    )}
                  </div>
                </div>
              ))}
            </SettingCard>
          )}
        </SettingsSection>

        {/* ─── Search & Forget ─── */}
        <SettingsSection
          icon={Search}
          title={t("search")}
          description={t("searchPlaceholder")}
        >
          <form onSubmit={handleSearch} className="flex gap-2">
            <div className="relative flex-1">
              <Search className="absolute left-2.5 top-2.5 size-4 text-muted-foreground" />
              <Input
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                placeholder={t("searchPlaceholder")}
                className="pl-9"
                disabled={backend === "off"}
              />
            </div>
            <Button
              type="submit"
              disabled={!searchQuery.trim() || searching || backend === "off"}
            >
              {searching ? (
                <Loader2 className="size-3.5 animate-spin" />
              ) : (
                t("search")
              )}
            </Button>
          </form>

          {searchHits !== null && (
            <div className="mt-3 space-y-2">
              {searchHits.length === 0 ? (
                <div className="rounded-xl border border-dashed border-border/70 p-6 text-center text-xs text-muted-foreground">
                  {t("empty")}
                </div>
              ) : (
                <div className="space-y-2">
                  {searchHits.map((hit) => (
                    <div
                      key={hit.node.id}
                      className="rounded-xl border border-border/70 bg-card p-3 space-y-2"
                    >
                      <div className="flex items-start justify-between gap-2">
                        <div className="space-y-1">
                          <div className="flex flex-wrap items-center gap-1.5">
                            <span className="text-xs font-semibold">
                              {hit.node.title}
                            </span>
                            <Badge variant="outline" className="text-3xs">
                              {hit.node.kind}
                            </Badge>
                            {hit.node.provenance.verified_by_tests && (
                              <Badge
                                variant="secondary"
                                className="text-3xs text-emerald-600 dark:text-emerald-400"
                              >
                                {t("verifiedByTests")}
                              </Badge>
                            )}
                            {hit.node.stale_at && (
                              <Badge variant="destructive" className="text-3xs">
                                {t("stale")}
                              </Badge>
                            )}
                          </div>
                          {(hit.node.provenance.run_id ||
                            hit.node.provenance.step_id) && (
                            <p className="text-2xs text-muted-foreground">
                              {t("provenance", {
                                run: hit.node.provenance.run_id ?? "-",
                                step: hit.node.provenance.step_id ?? "-",
                              })}
                            </p>
                          )}
                        </div>

                        <Button
                          variant="ghost"
                          size="sm"
                          className="h-7 text-xs text-muted-foreground hover:text-destructive"
                          onClick={() => handleForgetNode(hit.node.id)}
                        >
                          <Trash2 className="size-3 mr-1" />
                          {t("deleteNode")}
                        </Button>
                      </div>

                      <p className="text-xs text-muted-foreground leading-relaxed whitespace-pre-wrap">
                        {hit.node.body}
                      </p>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </SettingsSection>
      </div>

      <MemoryKindDialog
        open={kindDialogOpen}
        onOpenChange={(open) => {
          setKindDialogOpen(open)
          if (!open) setEditingKind(null)
        }}
        editingKind={editingKind}
        onSave={handleKindSave}
        saving={savingKind}
      />
    </ScrollArea>
  )
}
