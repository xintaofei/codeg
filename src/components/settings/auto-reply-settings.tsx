"use client"

import { useState } from "react"
import { ArrowDown, ArrowUp, Loader2, Plus, Save, Trash2 } from "lucide-react"
import { useTranslations } from "next-intl"
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
import { Textarea } from "@/components/ui/textarea"
import { cn, randomUUID } from "@/lib/utils"
import {
  useAutoReplySettings,
  updateAutoReplySettings,
} from "@/lib/auto-reply/settings-store"
import type { AutoReplyMatchKind, AutoReplyRule } from "@/lib/auto-reply/types"

const CONTINUE = "\u7ee7\u7eed"

function cloneRules(rules: AutoReplyRule[]): AutoReplyRule[] {
  return rules.map((rule) => ({ ...rule }))
}

function createCustomRule(name: string): AutoReplyRule {
  return {
    id: `custom-${randomUUID()}`,
    name,
    enabled: true,
    matchKind: "error_text",
    matchValue: "",
    replyText: CONTINUE,
    delayMs: 3000,
    cooldownMs: 15000,
    maxPerBurst: 3,
  }
}

export function AutoReplySettings() {
  const t = useTranslations("AutoReplySettings")
  const stored = useAutoReplySettings()
  const [rules, setRules] = useState<AutoReplyRule[]>(() =>
    cloneRules(stored.rules)
  )
  const [selectedId, setSelectedId] = useState<string | null>(
    () => stored.rules[0]?.id ?? null
  )
  const [saving, setSaving] = useState(false)
  const [deleteTargetId, setDeleteTargetId] = useState<string | null>(null)

  const selected = rules.find((rule) => rule.id === selectedId) ?? null

  const updateSelected = (patch: Partial<AutoReplyRule>) => {
    if (!selected) return
    setRules((prev) =>
      prev.map((rule) =>
        rule.id === selected.id
          ? {
              ...rule,
              ...patch,
              id: rule.id,
              builtin: rule.builtin,
            }
          : rule
      )
    )
  }

  const saveOnce = () => {
    setSaving(true)
    try {
      const next = updateAutoReplySettings({ version: 1, rules })
      setRules(cloneRules(next.rules))
      if (!next.rules.some((r) => r.id === selectedId)) {
        setSelectedId(next.rules[0]?.id ?? null)
      }
      toast.success(t("saved"))
    } finally {
      setSaving(false)
    }
  }

  const handleAdd = () => {
    const rule = createCustomRule(t("newRuleName"))
    setRules((prev) => [...prev, rule])
    setSelectedId(rule.id)
  }

  const handleDelete = () => {
    if (!deleteTargetId) return
    const target = rules.find((r) => r.id === deleteTargetId)
    if (!target || target.builtin) {
      setDeleteTargetId(null)
      return
    }
    const next = rules.filter((r) => r.id !== deleteTargetId)
    setRules(next)
    if (selectedId === deleteTargetId) {
      setSelectedId(next[0]?.id ?? null)
    }
    setDeleteTargetId(null)
  }

  const move = (id: string, direction: -1 | 1) => {
    setRules((prev) => {
      const index = prev.findIndex((r) => r.id === id)
      if (index < 0) return prev
      const nextIndex = index + direction
      if (nextIndex < 0 || nextIndex >= prev.length) return prev
      const copy = [...prev]
      const [item] = copy.splice(index, 1)
      copy.splice(nextIndex, 0, item)
      return copy
    })
  }

  return (
    <div className="mx-auto flex w-full max-w-5xl flex-col gap-4 p-4 md:p-6">
      <div className="space-y-1">
        <h1 className="text-xl font-semibold">{t("title")}</h1>
        <p className="text-sm text-muted-foreground">{t("description")}</p>
        <p className="text-xs text-muted-foreground">{t("help")}</p>
      </div>

      <div className="flex flex-wrap items-center gap-2">
        <Button type="button" onClick={handleAdd}>
          <Plus className="size-4" />
          {t("addRule")}
        </Button>
        <Button
          type="button"
          variant="secondary"
          onClick={saveOnce}
          disabled={saving}
        >
          {saving ? (
            <Loader2 className="size-4 animate-spin" />
          ) : (
            <Save className="size-4" />
          )}
          {t("save")}
        </Button>
      </div>

      <div className="grid gap-4 md:grid-cols-[minmax(240px,280px)_1fr]">
        <div className="space-y-2 rounded-lg border p-2">
          {rules.length === 0 ? (
            <div className="p-3 text-sm text-muted-foreground">
              {t("empty")}
            </div>
          ) : (
            rules.map((rule, index) => (
              <button
                key={rule.id}
                type="button"
                onClick={() => setSelectedId(rule.id)}
                className={cn(
                  "flex w-full items-start justify-between gap-2 rounded-md border px-2.5 py-2 text-left text-sm transition-colors",
                  selectedId === rule.id
                    ? "border-primary/60 bg-primary/5"
                    : "hover:bg-muted/50"
                )}
              >
                <div className="min-w-0">
                  <div className="truncate font-medium">
                    {rule.name || rule.id}
                  </div>
                  <div className="truncate text-xs text-muted-foreground">
                    {rule.matchKind === "http_status"
                      ? `HTTP ${rule.matchValue}`
                      : rule.matchValue || rule.matchKind}
                  </div>
                  {rule.builtin ? (
                    <div className="mt-1 text-[10px] uppercase tracking-wide text-muted-foreground">
                      {t("builtinBadge")}
                    </div>
                  ) : null}
                </div>
                <div className="flex shrink-0 flex-col gap-1">
                  <Button
                    type="button"
                    size="icon-xs"
                    variant="ghost"
                    disabled={index === 0}
                    onClick={(e) => {
                      e.stopPropagation()
                      move(rule.id, -1)
                    }}
                    title={t("moveUp")}
                  >
                    <ArrowUp className="size-3.5" />
                  </Button>
                  <Button
                    type="button"
                    size="icon-xs"
                    variant="ghost"
                    disabled={index === rules.length - 1}
                    onClick={(e) => {
                      e.stopPropagation()
                      move(rule.id, 1)
                    }}
                    title={t("moveDown")}
                  >
                    <ArrowDown className="size-3.5" />
                  </Button>
                </div>
              </button>
            ))
          )}
        </div>

        <div className="rounded-lg border p-4">
          {!selected ? (
            <div className="text-sm text-muted-foreground">{t("empty")}</div>
          ) : (
            <div className="space-y-4">
              <div className="flex items-center justify-between gap-3">
                <div className="space-y-1">
                  <Label htmlFor="auto-reply-enabled">{t("enabled")}</Label>
                </div>
                <Switch
                  id="auto-reply-enabled"
                  checked={selected.enabled}
                  onCheckedChange={(checked) =>
                    updateSelected({ enabled: checked })
                  }
                />
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="auto-reply-name">{t("name")}</Label>
                <Input
                  id="auto-reply-name"
                  value={selected.name}
                  onChange={(e) => updateSelected({ name: e.target.value })}
                />
              </div>

              <div className="grid gap-3 sm:grid-cols-2">
                <div className="space-y-1.5">
                  <Label>{t("matchKind")}</Label>
                  <Select
                    value={selected.matchKind}
                    onValueChange={(value) =>
                      updateSelected({
                        matchKind: value as AutoReplyMatchKind,
                      })
                    }
                  >
                    <SelectTrigger>
                      <SelectValue />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="http_status">
                        {t("matchKindHttpStatus")}
                      </SelectItem>
                      <SelectItem value="error_text">
                        {t("matchKindErrorText")}
                      </SelectItem>
                    </SelectContent>
                  </Select>
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="auto-reply-match-value">
                    {t("matchValue")}
                  </Label>
                  <Input
                    id="auto-reply-match-value"
                    value={selected.matchValue}
                    onChange={(e) =>
                      updateSelected({ matchValue: e.target.value })
                    }
                  />
                </div>
              </div>

              <div className="space-y-1.5">
                <Label htmlFor="auto-reply-text">{t("replyText")}</Label>
                <Textarea
                  id="auto-reply-text"
                  value={selected.replyText}
                  onChange={(e) =>
                    updateSelected({ replyText: e.target.value })
                  }
                  rows={3}
                />
              </div>

              <div className="grid gap-3 sm:grid-cols-3">
                <div className="space-y-1.5">
                  <Label htmlFor="auto-reply-delay">{t("delaySeconds")}</Label>
                  <Input
                    id="auto-reply-delay"
                    type="number"
                    min={0}
                    step={0.5}
                    value={selected.delayMs / 1000}
                    onChange={(e) => {
                      const seconds = Number(e.target.value)
                      if (!Number.isFinite(seconds)) return
                      updateSelected({
                        delayMs: Math.max(0, Math.round(seconds * 1000)),
                      })
                    }}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="auto-reply-cooldown">
                    {t("cooldownSeconds")}
                  </Label>
                  <Input
                    id="auto-reply-cooldown"
                    type="number"
                    min={0}
                    step={0.5}
                    value={selected.cooldownMs / 1000}
                    onChange={(e) => {
                      const seconds = Number(e.target.value)
                      if (!Number.isFinite(seconds)) return
                      updateSelected({
                        cooldownMs: Math.max(0, Math.round(seconds * 1000)),
                      })
                    }}
                  />
                </div>
                <div className="space-y-1.5">
                  <Label htmlFor="auto-reply-max">{t("maxPerBurst")}</Label>
                  <Input
                    id="auto-reply-max"
                    type="number"
                    min={1}
                    step={1}
                    value={selected.maxPerBurst}
                    onChange={(e) => {
                      const n = Number(e.target.value)
                      if (!Number.isFinite(n)) return
                      updateSelected({
                        maxPerBurst: Math.max(1, Math.trunc(n)),
                      })
                    }}
                  />
                </div>
              </div>

              {selected.builtin ? (
                <p className="text-xs text-muted-foreground">
                  {t("cannotDeleteBuiltin")}
                </p>
              ) : (
                <Button
                  type="button"
                  variant="destructive"
                  onClick={() => setDeleteTargetId(selected.id)}
                >
                  <Trash2 className="size-4" />
                  {t("delete")}
                </Button>
              )}
            </div>
          )}
        </div>
      </div>

      <AlertDialog
        open={deleteTargetId != null}
        onOpenChange={(open) => {
          if (!open) setDeleteTargetId(null)
        }}
      >
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("confirmDeleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>
              {t("confirmDeleteDescription")}
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("cancel")}</AlertDialogCancel>
            <AlertDialogAction onClick={handleDelete}>
              {t("delete")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  )
}
