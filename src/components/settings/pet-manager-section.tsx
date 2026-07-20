"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import {
  ChevronDown,
  Import,
  Loader2,
  PawPrint,
  Pencil,
  Plus,
  Store,
  Trash2,
} from "lucide-react"
import { toast } from "sonner"
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
import { Button } from "@/components/ui/button"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { Popover, PopoverAnchor, PopoverContent } from "@/components/ui/popover"
import {
  isCodexImportAvailable,
  listPets,
  readPetSpritesheet,
  setActivePet,
  deletePet,
  openPetWindow,
  getPetSettings,
} from "@/lib/pet/api"
import { isDesktop } from "@/lib/transport"
import type { PetSummary } from "@/lib/pet/types"
import {
  createPetSpriteObjectUrl,
  revokePetSpriteObjectUrl,
} from "@/lib/pet/sprite-url"
import {
  backgroundPositionFor,
  spriteBackgroundSize,
  spriteRowsFromHeight,
} from "@/lib/pet/animation"
import { useImageNaturalSize } from "@/lib/pet/use-image-natural-size"
import { PetEditor } from "./pet-editor"
import { PetImporter } from "./pet-importer"
import { PetActionPreviewGrid } from "./pet-action-preview-grid"
import { PetMarketplaceDialog } from "./pet-marketplace-dialog"

const SPRITE_PREVIEW_CONCURRENCY = 4

export function PetManagerSection() {
  const t = useTranslations("Pet.manager")
  const tMarket = useTranslations("Pet.marketplace")
  const [pets, setPets] = useState<PetSummary[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [editorOpen, setEditorOpen] = useState(false)
  const [editorTarget, setEditorTarget] = useState<PetSummary | null>(null)
  const [importOpen, setImportOpen] = useState(false)
  const [marketplaceOpen, setMarketplaceOpen] = useState(false)
  const [codexAvailable, setCodexAvailable] = useState(false)
  const [deleteTarget, setDeleteTarget] = useState<PetSummary | null>(null)
  const [deletingId, setDeletingId] = useState<string | null>(null)
  const [expanded, setExpanded] = useState(false)
  const [previewPetId, setPreviewPetId] = useState<string | null>(null)
  const [sheetUrls, setSheetUrls] = useState<Record<string, string>>({})
  const sheetUrlsRef = useRef<Record<string, string>>({})
  const mountedRef = useRef(true)
  const refreshSeqRef = useRef(0)

  const replaceSheetUrls = useCallback((next: Record<string, string>) => {
    revokeSpriteUrlRecord(sheetUrlsRef.current)
    sheetUrlsRef.current = next
    setSheetUrls(next)
  }, [])

  const refresh = useCallback(async () => {
    const seq = refreshSeqRef.current + 1
    refreshSeqRef.current = seq
    await Promise.resolve()
    if (!mountedRef.current || seq !== refreshSeqRef.current) return
    setLoading(true)
    try {
      const [list, settings, importerAvail] = await Promise.all([
        listPets(),
        getPetSettings().catch(() => null),
        isCodexImportAvailable().catch(() => ({ available: false })),
      ])
      if (!mountedRef.current || seq !== refreshSeqRef.current) return
      setPets(list)
      setActiveId(settings?.activePetId ?? null)
      setCodexAvailable(importerAvail.available)
      setPreviewPetId(null)
      replaceSheetUrls({})
      setLoading(false)

      void loadSpritePreviews(list).then((nextSheetUrls) => {
        if (!mountedRef.current || seq !== refreshSeqRef.current) {
          revokeSpriteUrlRecord(nextSheetUrls)
          return
        }
        replaceSheetUrls(nextSheetUrls)
      })
    } catch (err) {
      if (mountedRef.current && seq === refreshSeqRef.current) {
        toast.error(t("errors.loadFailed"), { description: toMessage(err) })
        setLoading(false)
      }
    }
  }, [replaceSheetUrls, t])

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
      refreshSeqRef.current += 1
      revokeSpriteUrlRecord(sheetUrlsRef.current)
      sheetUrlsRef.current = {}
    }
  }, [])

  useEffect(() => {
    const timer = window.setTimeout(() => {
      void refresh()
    }, 0)
    return () => window.clearTimeout(timer)
  }, [refresh])

  const handleSetActive = useCallback(
    async (petId: string) => {
      try {
        const next = await setActivePet(petId)
        setActiveId(next.activePetId)
      } catch (err) {
        toast.error(t("errors.setActiveFailed"), {
          description: toMessage(err),
        })
      }
    },
    [t]
  )

  const handleConfirmDelete = useCallback(async () => {
    if (!deleteTarget) return
    const pet = deleteTarget
    setDeletingId(pet.id)
    try {
      await deletePet(pet.id)
      setDeleteTarget(null)
      await refresh()
    } catch (err) {
      toast.error(t("errors.deleteFailed"), { description: toMessage(err) })
    } finally {
      setDeletingId(null)
    }
  }, [deleteTarget, refresh, t])

  const openEditor = useCallback((target: PetSummary | null) => {
    setEditorTarget(target)
    setEditorOpen(true)
  }, [])

  const handleSummon = useCallback(async () => {
    if (!isDesktop()) return
    try {
      await openPetWindow()
    } catch (err) {
      toast.error(t("errors.summonFailed"), { description: toMessage(err) })
    }
  }, [t])

  const summonDisabled = !isDesktop() || !activeId

  const installedIds = useMemo(() => new Set(pets.map((p) => p.id)), [pets])

  return (
    <Collapsible
      open={expanded}
      onOpenChange={setExpanded}
      className="overflow-hidden rounded-xl border bg-card"
      asChild
    >
      <section>
        <CollapsibleTrigger asChild>
          <div
            role="button"
            tabIndex={0}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault()
                setExpanded((v) => !v)
              }
            }}
            className="group/pet-toggle flex flex-wrap items-center justify-between gap-3 border-b border-transparent p-4 transition-colors hover:bg-muted/50 cursor-pointer outline-none focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-ring/50 data-[state=open]:border-border"
          >
            <div className="flex min-w-0 items-center gap-2">
              <PawPrint className="h-4 w-4 shrink-0 text-muted-foreground" />
              <h2 className="text-sm font-semibold">{t("title")}</h2>
              {pets.length > 0 ? (
                <span className="text-xs text-muted-foreground">
                  ({pets.length})
                </span>
              ) : null}
              <ChevronDown className="h-3.5 w-3.5 text-muted-foreground transition-transform duration-200 group-data-[state=open]/pet-toggle:rotate-180" />
            </div>
            <div
              className="flex flex-wrap items-center gap-2"
              onClick={(e) => e.stopPropagation()}
              onKeyDown={(e) => e.stopPropagation()}
            >
              {isDesktop() ? (
                <Button
                  type="button"
                  variant="default"
                  size="xs"
                  onClick={handleSummon}
                  disabled={summonDisabled}
                  title={!activeId ? t("noPets") : undefined}
                >
                  <PawPrint className="mr-1 h-3.5 w-3.5" />
                  {t("summon")}
                </Button>
              ) : null}
              <Button
                type="button"
                variant="outline"
                size="icon-xs"
                onClick={() => openEditor(null)}
                title={t("addPet")}
                aria-label={t("addPet")}
              >
                <Plus className="h-3.5 w-3.5" />
              </Button>
              <Button
                type="button"
                variant="outline"
                size="icon-xs"
                onClick={() => setImportOpen(true)}
                disabled={!codexAvailable}
                title={
                  !codexAvailable ? t("openCodexHelp") : t("importFromCodex")
                }
                aria-label={t("importFromCodex")}
              >
                <Import className="h-3.5 w-3.5" />
              </Button>
              <Button
                type="button"
                variant="outline"
                size="icon-xs"
                onClick={() => setMarketplaceOpen(true)}
                title={tMarket("openMarketplace")}
                aria-label={tMarket("openMarketplace")}
              >
                <Store className="h-3.5 w-3.5" />
              </Button>
            </div>
          </div>
        </CollapsibleTrigger>

        <CollapsibleContent className="space-y-4 p-4">
          {loading ? (
            <div className="flex items-center gap-2 text-sm text-muted-foreground">
              <Loader2 className="h-4 w-4 animate-spin" />
            </div>
          ) : pets.length === 0 ? (
            <div className="rounded-md border border-dashed border-border bg-muted/30 px-4 py-6 text-center text-sm text-muted-foreground">
              <PawPrint className="mx-auto mb-2 h-6 w-6 opacity-60" />
              <div>{t("noPets")}</div>
            </div>
          ) : (
            <div className="grid grid-cols-1 gap-3 md:grid-cols-2 xl:grid-cols-3">
              {pets.map((pet) => {
                const active = pet.id === activeId
                const sheetUrl = sheetUrls[pet.id] ?? ""
                const previewOpen = previewPetId === pet.id && Boolean(sheetUrl)
                return (
                  <Popover
                    key={pet.id}
                    open={previewOpen}
                    onOpenChange={(nextOpen) =>
                      setPreviewPetId(nextOpen && sheetUrl ? pet.id : null)
                    }
                  >
                    <PopoverAnchor asChild>
                      <div
                        onClick={() => {
                          if (!sheetUrl) return
                          setPreviewPetId(previewOpen ? null : pet.id)
                        }}
                        className={`cursor-pointer rounded-lg border p-3 transition-colors ${
                          active
                            ? "border-primary bg-primary/5"
                            : "border-border hover:border-primary/40"
                        } ${previewOpen ? "ring-1 ring-primary/30" : ""}`}
                      >
                        <button
                          type="button"
                          disabled={!sheetUrl}
                          aria-expanded={previewOpen}
                          onClick={(event) => {
                            event.stopPropagation()
                            if (!sheetUrl) return
                            setPreviewPetId(previewOpen ? null : pet.id)
                          }}
                          className="flex w-full items-start gap-3 rounded-md text-left outline-none focus-visible:ring-2 focus-visible:ring-ring/50 focus-visible:ring-offset-2 focus-visible:ring-offset-background disabled:cursor-default"
                        >
                          <PetSheetThumbnail url={sheetUrl} />
                          <div className="min-w-0 flex-1">
                            <div
                              className="truncate text-sm font-medium"
                              title={pet.displayName}
                            >
                              {pet.displayName}
                            </div>
                            {pet.description ? (
                              <div
                                className="mt-1 line-clamp-2 text-xs text-muted-foreground"
                                title={pet.description}
                              >
                                {pet.description}
                              </div>
                            ) : null}
                          </div>
                        </button>
                        <div
                          className="mt-3 flex flex-nowrap items-center gap-2"
                          onClick={(event) => event.stopPropagation()}
                        >
                          <Button
                            size="sm"
                            variant="outline"
                            type="button"
                            className="min-w-0 flex-1"
                            onClick={() => handleSetActive(pet.id)}
                            disabled={active}
                          >
                            <span className="truncate">
                              {active ? t("active") : t("setActive")}
                            </span>
                          </Button>
                          <Button
                            size="icon-sm"
                            variant="ghost"
                            type="button"
                            onClick={() => openEditor(pet)}
                            title={t("edit")}
                            aria-label={t("edit")}
                          >
                            <Pencil className="h-3.5 w-3.5" />
                          </Button>
                          <Button
                            size="icon-sm"
                            variant="ghost"
                            type="button"
                            className="text-destructive hover:bg-destructive/10"
                            onClick={() => setDeleteTarget(pet)}
                            disabled={active}
                            title={t("delete")}
                            aria-label={t("delete")}
                          >
                            <Trash2 className="h-3.5 w-3.5" />
                          </Button>
                        </div>
                      </div>
                    </PopoverAnchor>
                    {sheetUrl ? (
                      <PopoverContent
                        side="right"
                        align="start"
                        sideOffset={10}
                        collisionPadding={12}
                        className="z-[60] w-72 rounded-lg p-2"
                      >
                        <PetActionPreviewGrid
                          petName={pet.displayName}
                          source={{ type: "spritesheet", url: sheetUrl }}
                        />
                      </PopoverContent>
                    ) : null}
                  </Popover>
                )
              })}
            </div>
          )}
        </CollapsibleContent>

        <PetEditor
          open={editorOpen}
          target={editorTarget}
          onClose={() => setEditorOpen(false)}
          onSaved={async () => {
            setEditorOpen(false)
            await refresh()
          }}
        />

        <PetImporter
          open={importOpen}
          onClose={() => setImportOpen(false)}
          onDone={async () => {
            setImportOpen(false)
            await refresh()
          }}
        />

        <PetMarketplaceDialog
          open={marketplaceOpen}
          onOpenChange={setMarketplaceOpen}
          installedIds={installedIds}
          onInstalled={refresh}
        />

        <AlertDialog
          open={Boolean(deleteTarget)}
          onOpenChange={(open) => {
            if (!open && !deletingId) setDeleteTarget(null)
          }}
        >
          <AlertDialogContent size="sm">
            <AlertDialogHeader>
              <AlertDialogTitle>{t("delete")}</AlertDialogTitle>
              <AlertDialogDescription>
                {t("deleteConfirm", { name: deleteTarget?.displayName ?? "" })}
              </AlertDialogDescription>
            </AlertDialogHeader>
            <AlertDialogFooter>
              <AlertDialogCancel disabled={Boolean(deletingId)}>
                {t("form.cancel")}
              </AlertDialogCancel>
              <AlertDialogAction
                variant="destructive"
                disabled={Boolean(deletingId)}
                onClick={(e) => {
                  e.preventDefault()
                  void handleConfirmDelete()
                }}
              >
                {t("delete")}
              </AlertDialogAction>
            </AlertDialogFooter>
          </AlertDialogContent>
        </AlertDialog>
      </section>
    </Collapsible>
  )
}
async function loadSpritePreviews(
  pets: PetSummary[]
): Promise<Record<string, string>> {
  const results: Array<readonly [string, string]> = []
  let nextIndex = 0

  async function worker() {
    while (nextIndex < pets.length) {
      const index = nextIndex
      nextIndex += 1
      const pet = pets[index]
      if (!pet) continue
      results.push(await loadSpritePreview(pet))
    }
  }

  const workerCount = Math.min(SPRITE_PREVIEW_CONCURRENCY, pets.length)
  await Promise.all(Array.from({ length: workerCount }, () => worker()))

  return Object.fromEntries(results)
}

/** Static first-frame (idle) thumbnail. Measures the sheet so v2 (11-row) pets
 *  don't bleed the next row into the frame under the legacy 9-row scaling. */
function PetSheetThumbnail({ url }: { url: string }) {
  const size = useImageNaturalSize(url || null)
  const rows = spriteRowsFromHeight(size?.height)
  return (
    <div className="flex h-14 w-14 shrink-0 items-center justify-center rounded-md border border-border bg-background p-1.5">
      <div
        className="h-full"
        style={{
          aspectRatio: "192 / 208",
          backgroundImage: url ? `url("${url}")` : undefined,
          backgroundSize: spriteBackgroundSize(rows),
          backgroundPosition: backgroundPositionFor(0, 0, rows),
          backgroundRepeat: "no-repeat",
          imageRendering: "pixelated",
        }}
      />
    </div>
  )
}

async function loadSpritePreview(
  pet: PetSummary
): Promise<readonly [string, string]> {
  try {
    const asset = await readPetSpritesheet(pet.id)
    return [pet.id, createPetSpriteObjectUrl(asset)] as const
  } catch {
    return [pet.id, ""] as const
  }
}

function revokeSpriteUrlRecord(urls: Record<string, string>): void {
  for (const url of Object.values(urls)) {
    revokePetSpriteObjectUrl(url)
  }
}

function toMessage(err: unknown): string {
  if (err instanceof Error) return err.message
  if (typeof err === "string") return err
  if (err && typeof err === "object" && "message" in err) {
    const m = (err as { message: unknown }).message
    if (typeof m === "string") return m
  }
  return String(err)
}
