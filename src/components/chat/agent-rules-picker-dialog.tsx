"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { AlertTriangle, RefreshCw, Trash2 } from "lucide-react"

import {
  agentRulesDeleteProfile,
  agentRulesInspect,
  agentRulesRender,
  agentRulesRenameProfile,
  agentRulesSaveProfile,
} from "@/lib/api"
import { extractAppCommandError, toErrorMessage } from "@/lib/app-error"
import type {
  AgentRulesInspectResult,
  AgentRulesRenderResult,
  AgentType,
} from "@/lib/types"
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
import { ScrollArea } from "@/components/ui/scroll-area"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { getAgentLabel } from "@/lib/custom-agents"

import type { AgentRuleSelectionAttrs } from "./composer/agent-rule-selection"

export interface AgentRulesPickerDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  rootPath: string | null
  agentType: AgentType | null
  onApply: (attrs: AgentRuleSelectionAttrs) => void
}

const CATALOG_CHANGED_MESSAGE = "catalog changed"

function errorMessage(error: unknown): string {
  return extractAppCommandError(error)?.message ?? toErrorMessage(error)
}

export function AgentRulesPickerDialog({
  open,
  onOpenChange,
  rootPath,
  agentType,
  onApply,
}: AgentRulesPickerDialogProps) {
  const t = useTranslations("Folder.chat.messageInput.agentRulesPicker")
  const [catalog, setCatalog] = useState<AgentRulesInspectResult | null>(null)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const [preview, setPreview] = useState<AgentRulesRenderResult | null>(null)
  const [loading, setLoading] = useState(false)
  const [previewLoading, setPreviewLoading] = useState(false)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [profileName, setProfileName] = useState("")
  const [profileRevised, setProfileRevised] = useState(false)
  const [setDefault, setSetDefault] = useState(false)
  const profileInputRef = useRef<HTMLInputElement>(null)

  const load = useCallback(async () => {
    if (!rootPath) return
    setLoading(true)
    setError(null)
    setPreview(null)
    try {
      const next = await agentRulesInspect(rootPath)
      setCatalog(next)
      const defaultProfile = next.defaultProfile
        ? next.profiles[next.defaultProfile]
        : null
      setSelected(new Set(defaultProfile?.ruleIds ?? next.defaultIds))
      setProfileName(next.defaultProfile ?? "")
      setProfileRevised(false)
      setSetDefault(false)
    } catch (loadError) {
      setError(errorMessage(loadError))
    } finally {
      setLoading(false)
    }
  }, [rootPath])

  useEffect(() => {
    if (open) void load()
  }, [open, load])

  const selectedIds = useMemo(
    () =>
      catalog?.rules
        .filter((rule) => selected.has(rule.id))
        .map((rule) => rule.id) ?? [],
    [catalog, selected]
  )

  useEffect(() => {
    if (!open || !rootPath || !catalog) return
    let active = true
    const timeout = window.setTimeout(() => {
      setPreviewLoading(true)
      agentRulesRender({
        rootPath,
        ruleIds: selectedIds,
        expectedSourceHash: catalog.sourceHash,
      })
        .then((result) => {
          if (active) {
            setPreview(result)
            setError(null)
          }
        })
        .catch((previewError) => {
          if (active) setError(errorMessage(previewError))
        })
        .finally(() => {
          if (active) setPreviewLoading(false)
        })
    }, 120)
    return () => {
      active = false
      window.clearTimeout(timeout)
    }
  }, [open, rootPath, catalog, selectedIds])

  const groups = useMemo(() => {
    const grouped = new Map<string, NonNullable<typeof catalog>["rules"]>()
    for (const rule of catalog?.rules ?? []) {
      const group = grouped.get(rule.source) ?? []
      group.push(rule)
      grouped.set(rule.source, group)
    }
    return [...grouped.entries()]
  }, [catalog])

  const chooseProfile = useCallback(
    (name: string) => {
      if (!catalog) return
      if (name === "__defaults__") {
        setSelected(new Set(catalog.defaultIds))
        setProfileName("")
        setProfileRevised(false)
        return
      }
      const profile = catalog.profiles[name]
      if (!profile) return
      setSelected(new Set(profile.ruleIds))
      setProfileName(name)
      setProfileRevised(false)
    },
    [catalog]
  )

  const finalRender = useCallback(async () => {
    if (!rootPath || !catalog) return null
    try {
      return await agentRulesRender({
        rootPath,
        ruleIds: selectedIds,
        expectedSourceHash: catalog.sourceHash,
      })
    } catch (renderError) {
      const message = errorMessage(renderError)
      setError(message)
      if (message.toLowerCase().includes(CATALOG_CHANGED_MESSAGE)) await load()
      return null
    }
  }, [rootPath, catalog, selectedIds, load])

  const applyRendered = useCallback(
    (rendered: AgentRulesRenderResult) => {
      onApply({
        version: 1,
        ruleIds: rendered.rules.map((rule) => rule.id),
        sourceHash: rendered.sourceHash,
        sources: rendered.sources,
        exactText: rendered.text,
        envelopeNonce: rendered.envelopeNonce,
      })
      onOpenChange(false)
    },
    [onApply, onOpenChange]
  )

  const applyOnce = useCallback(async () => {
    setSaving(true)
    const rendered = await finalRender()
    setSaving(false)
    if (rendered) applyRendered(rendered)
  }, [applyRendered, finalRender])

  const saveAndApply = useCallback(async () => {
    if (!rootPath || !catalog) return
    const name = profileName.trim()
    if (!name) {
      setError(t("profileNameRequired"))
      profileInputRef.current?.focus()
      return
    }
    setSaving(true)
    try {
      let overwrite = false
      if (catalog.profiles[name]) {
        overwrite = window.confirm(t("overwriteProfile", { name }))
        if (!overwrite) return
      }
      await agentRulesSaveProfile({
        rootPath,
        name,
        ruleIds: selectedIds,
        expectedSourceHash: catalog.sourceHash,
        setDefault,
        overwrite,
      })
      const rendered = await finalRender()
      if (rendered) applyRendered(rendered)
    } catch (saveError) {
      setError(errorMessage(saveError))
    } finally {
      setSaving(false)
    }
  }, [
    rootPath,
    catalog,
    profileName,
    selectedIds,
    setDefault,
    finalRender,
    applyRendered,
    t,
  ])

  const renameProfile = useCallback(async () => {
    if (!rootPath || !catalog || !profileName || !catalog.profiles[profileName])
      return
    const newName = window.prompt(t("renamePrompt"), profileName)?.trim()
    if (!newName || newName === profileName) return
    try {
      const overwrite = Boolean(catalog.profiles[newName])
      if (
        overwrite &&
        !window.confirm(t("overwriteProfile", { name: newName }))
      )
        return
      const next = await agentRulesRenameProfile({
        rootPath,
        oldName: profileName,
        newName,
        overwrite,
      })
      setCatalog(next)
      setProfileName(newName)
    } catch (renameError) {
      setError(errorMessage(renameError))
    }
  }, [rootPath, catalog, profileName, t])

  const deleteProfile = useCallback(async () => {
    if (!rootPath || !catalog || !profileName || !catalog.profiles[profileName])
      return
    if (!window.confirm(t("deleteProfileConfirm", { name: profileName })))
      return
    try {
      const next = await agentRulesDeleteProfile({
        rootPath,
        name: profileName,
      })
      setCatalog(next)
      setSelected(new Set(next.defaultIds))
      setProfileName("")
      setProfileRevised(false)
    } catch (deleteError) {
      setError(errorMessage(deleteError))
    }
  }, [rootPath, catalog, profileName, t])

  const activeProfile = profileName ? catalog?.profiles[profileName] : null
  const missingIds = profileRevised ? [] : (activeProfile?.missingRuleIds ?? [])
  const applyDisabled =
    loading || previewLoading || saving || !catalog || missingIds.length > 0

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex max-h-[min(54rem,calc(100vh-2rem))] max-w-5xl flex-col overflow-hidden">
        <DialogHeader>
          <DialogTitle>{t("title")}</DialogTitle>
          <DialogDescription>{t("description")}</DialogDescription>
        </DialogHeader>

        {!rootPath ? (
          <div className="rounded-md border border-dashed p-6 text-sm text-muted-foreground">
            {t("workspaceRequired")}
          </div>
        ) : loading ? (
          <div
            className="flex min-h-64 items-center justify-center text-sm text-muted-foreground"
            aria-live="polite"
          >
            {t("loading")}
          </div>
        ) : (
          <div className="min-h-0 flex-1 space-y-3 overflow-y-auto pr-1">
            <div className="flex flex-wrap items-center justify-between gap-2 text-xs text-muted-foreground">
              <span className="truncate" title={rootPath}>
                {rootPath}
              </span>
              <span>
                {agentType ? getAgentLabel(agentType) : t("unknownAgent")}
              </span>
            </div>

            <div className="rounded-md border border-amber-500/30 bg-amber-500/5 p-3">
              <div className="flex items-center gap-2 text-sm font-medium">
                <AlertTriangle className="size-4" aria-hidden="true" />
                {t("alwaysOn")}
              </div>
              <p className="mt-1 text-xs text-muted-foreground">
                {t("alwaysOnDescription")}
              </p>
              <div className="mt-2 flex flex-wrap gap-1">
                {catalog?.nativeSources.length ? (
                  catalog.nativeSources.map((source) => (
                    <span
                      key={source}
                      className="rounded bg-muted px-1.5 py-0.5 font-mono text-xs"
                    >
                      {source}
                    </span>
                  ))
                ) : (
                  <span className="text-xs text-muted-foreground">
                    {t("noneDetected")}
                  </span>
                )}
              </div>
            </div>

            {error ? (
              <div
                role="alert"
                className="flex items-start justify-between gap-2 rounded-md border border-destructive/40 bg-destructive/5 p-3 text-sm text-destructive"
              >
                <span>{error}</span>
                <Button
                  type="button"
                  variant="ghost"
                  size="icon-xs"
                  onClick={() => void load()}
                  title={t("refresh")}
                >
                  <RefreshCw className="size-3.5" />
                </Button>
              </div>
            ) : null}

            {activeProfile?.stale ? (
              <p className="text-xs text-amber-700 dark:text-amber-300">
                {t("staleProfile")}
              </p>
            ) : null}
            {missingIds.length > 0 ? (
              <div className="flex flex-wrap items-center gap-2 text-xs text-destructive">
                <span>{t("missingRules", { ids: missingIds.join(", ") })}</span>
                <Button
                  type="button"
                  variant="outline"
                  size="sm"
                  onClick={() => {
                    setSelected(new Set(selectedIds))
                    setProfileRevised(true)
                  }}
                >
                  {t("removeMissing")}
                </Button>
              </div>
            ) : null}

            {catalog && catalog.rules.length === 0 ? (
              <div className="rounded-md border border-dashed p-5 text-sm">
                <p>{t("emptyCatalog")}</p>
                <pre className="mt-3 overflow-x-auto rounded bg-muted p-3 text-xs">{`<!-- codeg-rule id="tests" name="Testing" default="on" -->\nRun the relevant tests.\n<!-- /codeg-rule -->`}</pre>
              </div>
            ) : catalog ? (
              <>
                <div className="flex flex-wrap items-end gap-2">
                  <div className="min-w-48 flex-1 space-y-1">
                    <Label>{t("profile")}</Label>
                    <Select
                      value={activeProfile ? profileName : "__defaults__"}
                      onValueChange={chooseProfile}
                    >
                      <SelectTrigger>
                        <SelectValue />
                      </SelectTrigger>
                      <SelectContent>
                        <SelectItem value="__defaults__">
                          {t("catalogDefaults")}
                        </SelectItem>
                        {Object.keys(catalog.profiles).map((name) => (
                          <SelectItem key={name} value={name}>
                            {name}
                          </SelectItem>
                        ))}
                      </SelectContent>
                    </Select>
                  </div>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    disabled={!activeProfile}
                    onClick={() => void renameProfile()}
                  >
                    {t("rename")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="icon-sm"
                    disabled={!activeProfile}
                    onClick={() => void deleteProfile()}
                    title={t("deleteProfile")}
                  >
                    <Trash2 className="size-4" />
                  </Button>
                </div>

                <div className="flex flex-wrap gap-2">
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      setSelected(new Set(catalog.defaultIds))
                      setProfileRevised(true)
                    }}
                  >
                    {t("defaults")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      setSelected(new Set(catalog.rules.map((rule) => rule.id)))
                      setProfileRevised(true)
                    }}
                  >
                    {t("selectAll")}
                  </Button>
                  <Button
                    type="button"
                    variant="outline"
                    size="sm"
                    onClick={() => {
                      setSelected(new Set())
                      setProfileRevised(true)
                    }}
                  >
                    {t("clear")}
                  </Button>
                  <span
                    className="self-center text-xs text-muted-foreground"
                    aria-live="polite"
                  >
                    {t("selectedCount", {
                      selected: selectedIds.length,
                      total: catalog.rules.length,
                    })}
                  </span>
                </div>

                <div className="grid min-h-72 gap-3 md:grid-cols-2">
                  <ScrollArea className="h-80 rounded-md border">
                    <div className="space-y-4 p-3">
                      {groups.map(([source, rules]) => (
                        <section key={source}>
                          <h3 className="mb-2 font-mono text-xs font-semibold">
                            {source}
                          </h3>
                          <div className="space-y-2">
                            {rules.map((rule) => (
                              <label
                                key={rule.id}
                                className="flex cursor-pointer items-start gap-2 rounded-md p-2 hover:bg-muted/60"
                              >
                                <Checkbox
                                  checked={selected.has(rule.id)}
                                  onCheckedChange={(checked) => {
                                    setProfileRevised(true)
                                    setSelected((current) => {
                                      const next = new Set(current)
                                      if (checked) next.add(rule.id)
                                      else next.delete(rule.id)
                                      return next
                                    })
                                  }}
                                  aria-label={rule.name}
                                />
                                <span className="min-w-0">
                                  <span className="block text-sm font-medium">
                                    {rule.name}
                                  </span>
                                  <span className="block font-mono text-xs text-muted-foreground">
                                    {rule.id} · {t("line", { line: rule.line })}
                                  </span>
                                </span>
                              </label>
                            ))}
                          </div>
                        </section>
                      ))}
                    </div>
                  </ScrollArea>
                  <div className="flex h-80 min-w-0 flex-col rounded-md border">
                    <div className="border-b px-3 py-2 text-xs font-semibold">
                      {t("exactPreview")}
                    </div>
                    <ScrollArea className="min-h-0 flex-1">
                      <pre className="whitespace-pre-wrap break-words p-3 text-xs">
                        {previewLoading
                          ? t("previewLoading")
                          : preview?.text || t("noOptionalRules")}
                      </pre>
                    </ScrollArea>
                  </div>
                </div>

                <div className="grid gap-2 rounded-md border p-3 sm:grid-cols-[1fr_auto]">
                  <div className="space-y-1">
                    <Label htmlFor="agent-rule-profile-name">
                      {t("profileName")}
                    </Label>
                    <Input
                      id="agent-rule-profile-name"
                      ref={profileInputRef}
                      value={profileName}
                      onChange={(event) => setProfileName(event.target.value)}
                      placeholder={t("profileNamePlaceholder")}
                    />
                  </div>
                  <label className="flex items-center gap-2 self-end pb-2 text-sm">
                    <Checkbox
                      checked={setDefault}
                      onCheckedChange={(checked) =>
                        setSetDefault(checked === true)
                      }
                    />
                    {t("makeDefault")}
                  </label>
                </div>
              </>
            ) : null}
          </div>
        )}

        <DialogFooter>
          <Button
            type="button"
            variant="outline"
            onClick={() => onOpenChange(false)}
          >
            {t("cancel")}
          </Button>
          <Button
            type="button"
            variant="outline"
            disabled={applyDisabled}
            onClick={() => void applyOnce()}
          >
            {t("applyOnce")}
          </Button>
          <Button
            type="button"
            disabled={applyDisabled}
            onClick={() => void saveAndApply()}
          >
            {t("saveAndApply")}
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
