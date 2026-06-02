"use client"

/**
 * Multi-agent delegation settings panel. Two top-level concerns split into
 * tabs:
 *
 *   * "General" — feature kill switch + chain depth limit. Persisted as
 *     `delegation.enabled` / `delegation.depth_limit` on the Rust side.
 *   * "Agent defaults" — per-agent overrides (mode + config_values) that
 *     codeg-mcp uses when spawning a subagent for a `delegate_to_agent`
 *     call. Persisted as `delegation.agent_defaults` (one JSON blob).
 *
 * Cancellation is handled out-of-band via MCP `notifications/cancelled`
 * forwarded from the parent agent CLI; there is no broker-side timeout to
 * configure here.
 *
 * Mounted under `/settings/general` next to the terminal and rendering
 * sections, because delegation is a global feature — not per-agent — and
 * doesn't belong inside the 7,800-line `acp-agent-settings.tsx` that
 * powers `/settings/agents`.
 */

import { useCallback, useEffect, useState } from "react"
import { useTranslations } from "next-intl"
import { Bubbles, Loader2 } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import {
  type DelegationSettings,
  getDelegationSettings,
  setDelegationSettings,
} from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import type { AgentDelegationDefaults, AgentType } from "@/lib/types"
import { DelegationAgentDefaultsPanel } from "./delegation-agent-defaults"

const DEPTH_MIN = 1
const DEPTH_MAX = 8
const DEFAULT_CACHE_MB = 512

function clamp(n: number, lo: number, hi: number): number {
  if (!Number.isFinite(n)) return lo
  return Math.min(hi, Math.max(lo, Math.trunc(n)))
}

/** Cache budget in MB: floor at 0 (= unlimited), drop fractional MB, no upper
 * bound (it's a memory choice, not a safety rail). NaN (cleared/garbage input)
 * falls back to the product default rather than silently disabling the valve. */
function clampCacheMb(n: number): number {
  if (!Number.isFinite(n)) return DEFAULT_CACHE_MB
  return Math.max(0, Math.trunc(n))
}

export function DelegationSettingsSection() {
  const t = useTranslations("AcpAgentSettings.multiAgent")
  const [loading, setLoading] = useState(true)
  const [saving, setSaving] = useState(false)
  const [enabled, setEnabled] = useState(false)
  const [depth, setDepth] = useState<number>(1)
  const [cacheMb, setCacheMb] = useState<number>(DEFAULT_CACHE_MB)
  const [agentDefaults, setAgentDefaults] = useState<
    Partial<Record<AgentType, AgentDelegationDefaults>>
  >({})
  const [loadError, setLoadError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void getDelegationSettings()
      .then((s) => {
        if (cancelled) return
        setEnabled(s.enabled)
        setDepth(s.depth_limit)
        setCacheMb(s.completed_cache_max_mb)
        setAgentDefaults(s.agent_defaults ?? {})
        setLoadError(null)
      })
      .catch((err: unknown) => {
        if (cancelled) return
        setLoadError(toErrorMessage(err))
      })
      .finally(() => {
        if (cancelled) return
        setLoading(false)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const save = useCallback(async () => {
    const payload: DelegationSettings = {
      enabled,
      depth_limit: clamp(depth, DEPTH_MIN, DEPTH_MAX),
      completed_cache_max_mb: clampCacheMb(cacheMb),
      agent_defaults: agentDefaults,
    }
    setSaving(true)
    try {
      const applied = await setDelegationSettings(payload)
      // Mirror any server-side clamps / filter passes back into the UI so the
      // inputs reflect what was actually persisted.
      setEnabled(applied.enabled)
      setDepth(applied.depth_limit)
      setCacheMb(applied.completed_cache_max_mb)
      setAgentDefaults(applied.agent_defaults ?? {})
      toast.success(t("saved"))
    } catch (err: unknown) {
      toast.error(t("saveFailed"), {
        description: toErrorMessage(err),
      })
    } finally {
      setSaving(false)
    }
  }, [enabled, depth, cacheMb, agentDefaults, t])

  return (
    <section className="rounded-xl border bg-card p-4 space-y-4">
      <div className="flex items-center gap-2">
        <Bubbles className="h-4 w-4 text-muted-foreground" aria-hidden />
        <h2 className="text-sm font-semibold">{t("title")}</h2>
      </div>
      <p className="text-xs text-muted-foreground leading-5">
        {t("description")}
      </p>

      {loadError && (
        <p className="rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2 text-xs text-destructive">
          {t("loadFailed", { detail: loadError })}
        </p>
      )}

      <Tabs defaultValue="general">
        <TabsList>
          <TabsTrigger value="general">{t("tabGeneral")}</TabsTrigger>
          <TabsTrigger value="agentDefaults">
            {t("tabAgentDefaults")}
          </TabsTrigger>
        </TabsList>

        <TabsContent value="general" className="space-y-4 pt-2">
          <div className="flex items-center justify-between gap-3">
            <div className="space-y-1 min-w-0">
              <label
                htmlFor="delegation-enabled"
                className="text-sm font-medium"
              >
                {t("enable")}
              </label>
              <p className="text-xs text-muted-foreground">{t("enableHint")}</p>
            </div>
            <Switch
              id="delegation-enabled"
              checked={enabled}
              onCheckedChange={setEnabled}
              disabled={loading}
              className="shrink-0"
            />
          </div>

          <div className="flex items-center justify-between gap-3">
            <div className="space-y-1 min-w-0">
              <label htmlFor="delegation-depth" className="text-sm font-medium">
                {t("depthLimit")}
              </label>
              <p className="text-xs text-muted-foreground">
                {t("depthHint", { min: DEPTH_MIN, max: DEPTH_MAX })}
              </p>
            </div>
            <Input
              id="delegation-depth"
              type="number"
              min={DEPTH_MIN}
              max={DEPTH_MAX}
              value={depth}
              onChange={(e) => setDepth(Number(e.target.value))}
              disabled={loading || !enabled}
              className="w-28 shrink-0"
            />
          </div>

          <div className="flex items-center justify-between gap-3">
            <div className="space-y-1 min-w-0">
              <label
                htmlFor="delegation-cache-mb"
                className="text-sm font-medium"
              >
                {t("completedCacheLabel")}
              </label>
              <p className="text-xs text-muted-foreground">
                {t("completedCacheHint")}
              </p>
            </div>
            <Input
              id="delegation-cache-mb"
              type="number"
              min={0}
              step={1}
              value={Number.isNaN(cacheMb) ? "" : cacheMb}
              onChange={(e) => {
                const raw = e.target.value
                // Empty (cleared) → NaN so `clampCacheMb` restores the default
                // on save, instead of `Number("") === 0` silently persisting
                // 0 (= unlimited). Explicit "0" still means unlimited.
                setCacheMb(raw === "" ? NaN : Number(raw))
              }}
              disabled={loading || !enabled}
              className="w-28 shrink-0"
            />
          </div>
        </TabsContent>

        <TabsContent value="agentDefaults" className="pt-2">
          <DelegationAgentDefaultsPanel
            value={agentDefaults}
            onChange={setAgentDefaults}
            disabled={loading || !enabled}
          />
        </TabsContent>
      </Tabs>

      <div className="flex justify-end pt-2">
        <Button onClick={save} disabled={loading || saving} size="sm">
          {saving ? (
            <>
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
              {t("saving")}
            </>
          ) : (
            t("save")
          )}
        </Button>
      </div>
    </section>
  )
}
