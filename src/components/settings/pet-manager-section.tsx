"use client"

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2, PawPrint, Plus, Sparkles, Trash2 } from "lucide-react"
import { Button } from "@/components/ui/button"
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
  SPRITE_BACKGROUND_SIZE,
  backgroundPositionFor,
} from "@/lib/pet/animation"
import { PetEditor } from "./pet-editor"
import { PetImporter } from "./pet-importer"

export function PetManagerSection() {
  const t = useTranslations("Pet.manager")
  const [pets, setPets] = useState<PetSummary[]>([])
  const [activeId, setActiveId] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [editorOpen, setEditorOpen] = useState(false)
  const [editorTarget, setEditorTarget] = useState<PetSummary | null>(null)
  const [importOpen, setImportOpen] = useState(false)
  const [codexAvailable, setCodexAvailable] = useState(false)
  const [sheetUrls, setSheetUrls] = useState<Record<string, string>>({})

  const refresh = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      const [list, settings, importerAvail] = await Promise.all([
        listPets(),
        getPetSettings().catch(() => null),
        isCodexImportAvailable().catch(() => ({ available: false })),
      ])
      setPets(list)
      setActiveId(settings?.activePetId ?? null)
      setCodexAvailable(importerAvail.available)

      // Webview can't load `file://` paths directly (and won't work in
      // server mode at all), so fetch each sheet through the existing
      // command and render from a base64 data URL.
      const sprites = await Promise.all(
        list.map(async (pet) => {
          try {
            const asset = await readPetSpritesheet(pet.id)
            return [
              pet.id,
              `data:${asset.mime};base64,${asset.dataBase64}`,
            ] as const
          } catch {
            return [pet.id, ""] as const
          }
        })
      )
      setSheetUrls(Object.fromEntries(sprites))
    } catch (err) {
      setError(toMessage(err))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void refresh()
  }, [refresh])

  const handleSetActive = useCallback(async (petId: string) => {
    try {
      const next = await setActivePet(petId)
      setActiveId(next.activePetId)
    } catch (err) {
      setError(toMessage(err))
    }
  }, [])

  const handleDelete = useCallback(
    async (pet: PetSummary) => {
      const ok = window.confirm(t("deleteConfirm"))
      if (!ok) return
      try {
        await deletePet(pet.id)
        await refresh()
      } catch (err) {
        setError(toMessage(err))
      }
    },
    [refresh, t]
  )

  const openEditor = useCallback((target: PetSummary | null) => {
    setEditorTarget(target)
    setEditorOpen(true)
  }, [])

  const handleSummon = useCallback(async () => {
    if (!isDesktop()) return
    try {
      await openPetWindow()
    } catch (err) {
      setError(toMessage(err))
    }
  }, [])

  const summonDisabled = !isDesktop() || !activeId

  return (
    <section className="rounded-xl border bg-card p-4 space-y-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <div className="flex items-center gap-2">
          <PawPrint className="h-4 w-4 text-muted-foreground" />
          <h2 className="text-sm font-semibold">{t("title")}</h2>
        </div>
        <div className="flex flex-wrap items-center gap-2">
          {isDesktop() ? (
            <Button
              type="button"
              variant="default"
              size="sm"
              onClick={handleSummon}
              disabled={summonDisabled}
              title={!activeId ? t("noPets") : undefined}
            >
              <PawPrint className="mr-1 h-4 w-4" />
              {t("summon")}
            </Button>
          ) : null}
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => openEditor(null)}
          >
            <Plus className="mr-1 h-4 w-4" />
            {t("addPet")}
          </Button>
          <Button
            type="button"
            variant="outline"
            size="sm"
            onClick={() => setImportOpen(true)}
            disabled={!codexAvailable}
            title={!codexAvailable ? t("openCodexHelp") : undefined}
          >
            <Sparkles className="mr-1 h-4 w-4" />
            {t("importFromCodex")}
          </Button>
        </div>
      </div>

      {error ? (
        <div className="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive">
          {error}
        </div>
      ) : null}

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
            return (
              <div
                key={pet.id}
                className={`rounded-lg border p-3 transition-colors ${
                  active
                    ? "border-primary bg-primary/5"
                    : "border-border hover:border-primary/40"
                }`}
              >
                <div className="flex items-start gap-3">
                  <div className="flex h-14 w-14 shrink-0 items-center justify-center rounded-md border border-border bg-background p-1.5">
                    <div
                      className="h-full"
                      style={{
                        aspectRatio: "192 / 208",
                        backgroundImage: sheetUrls[pet.id]
                          ? `url("${sheetUrls[pet.id]}")`
                          : undefined,
                        backgroundSize: SPRITE_BACKGROUND_SIZE,
                        backgroundPosition: backgroundPositionFor(0, 0),
                        backgroundRepeat: "no-repeat",
                        imageRendering: "pixelated",
                      }}
                    />
                  </div>
                  <div className="min-w-0 flex-1">
                    <div className="truncate text-sm font-medium">
                      {pet.displayName}
                    </div>
                    <div
                      className="truncate text-xs text-muted-foreground"
                      title={pet.id}
                    >
                      {pet.id}
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
                </div>
                <div className="mt-3 flex flex-wrap items-center gap-2">
                  {active ? (
                    <span className="rounded bg-primary/10 px-2 py-0.5 text-xs font-medium text-primary">
                      {t("active")}
                    </span>
                  ) : (
                    <Button
                      size="sm"
                      variant="outline"
                      type="button"
                      onClick={() => handleSetActive(pet.id)}
                    >
                      {t("setActive")}
                    </Button>
                  )}
                  <Button
                    size="sm"
                    variant="ghost"
                    type="button"
                    onClick={() => openEditor(pet)}
                  >
                    {t("edit")}
                  </Button>
                  <Button
                    size="sm"
                    variant="ghost"
                    type="button"
                    className="text-destructive hover:bg-destructive/10"
                    onClick={() => handleDelete(pet)}
                  >
                    <Trash2 className="mr-1 h-3.5 w-3.5" />
                    {t("delete")}
                  </Button>
                </div>
              </div>
            )
          })}
        </div>
      )}

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
    </section>
  )
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
