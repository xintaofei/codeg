"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { Loader2 } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Select,
  SelectContent,
  SelectGroup,
  SelectItem,
  SelectLabel,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { describeAgentOptions } from "@/lib/api"
import { toErrorMessage } from "@/lib/app-error"
import type {
  AgentOptionsSnapshot,
  AgentType,
  SessionConfigOptionInfo,
} from "@/lib/types"

// Picking this clears the override (inherit the agent's own default). Mirrors
// delegation-agent-defaults.tsx; the codeg prefix avoids colliding with a real
// option id.
const DEFAULT_SENTINEL = "__codeg_default__"
const CACHE_TTL_MS = 30_000

interface CachedSnapshot {
  snapshot: AgentOptionsSnapshot
  ts: number
}

// Module-scope probe cache, isolated from the chat selectors (same approach as
// delegation-agent-defaults). 30s TTL absorbs rapid re-opens without a stale
// snapshot surviving a real config change.
const snapshotCache = new Map<AgentType, CachedSnapshot>()

function readCache(agent: AgentType): AgentOptionsSnapshot | null {
  const entry = snapshotCache.get(agent)
  if (!entry) return null
  if (Date.now() - entry.ts > CACHE_TTL_MS) {
    snapshotCache.delete(agent)
    return null
  }
  return entry.snapshot
}

function writeCache(agent: AgentType, snapshot: AgentOptionsSnapshot): void {
  snapshotCache.set(agent, { snapshot, ts: Date.now() })
}

interface AgentConfigSectionProps {
  agentType: AgentType
  modeId: string | null
  configValues: Record<string, string>
  onModeChange: (modeId: string | null) => void
  onConfigChange: (optionId: string, valueId: string | null) => void
}

/**
 * The composer's model / mode / permission config surface, driven by a
 * side-effect-free probe (`describeAgentOptions`) rather than a live ACP
 * connection — what the user picks here is exactly what the automation run
 * replays. The model is one of the config options (id/category "model"); no
 * special-casing needed.
 */
export function AgentConfigSection({
  agentType,
  modeId,
  configValues,
  onModeChange,
  onConfigChange,
}: AgentConfigSectionProps) {
  const t = useTranslations("Automations")
  const [snapshot, setSnapshot] = useState<AgentOptionsSnapshot | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const reqRef = useRef(0)

  const load = useCallback(async (agent: AgentType, force: boolean) => {
    // Bump FIRST so a cache hit also invalidates any still-in-flight probe for a
    // previously-selected agent — otherwise that slow probe's late result would
    // overwrite the cached snapshot for the now-current agent.
    const id = ++reqRef.current
    if (!force) {
      const cached = readCache(agent)
      if (cached) {
        setSnapshot(cached)
        setError(null)
        setLoading(false)
        return
      }
    }
    setLoading(true)
    setError(null)
    setSnapshot(null)
    try {
      const fresh = await describeAgentOptions(agent)
      if (reqRef.current !== id) return
      writeCache(agent, fresh)
      setSnapshot(fresh)
    } catch (e) {
      if (reqRef.current !== id) return
      setError(toErrorMessage(e))
    } finally {
      if (reqRef.current === id) setLoading(false)
    }
  }, [])

  useEffect(() => {
    // Debounce so switching agents quickly doesn't fire a probe (CLI spawn) per
    // click; the last agent landed on wins.
    const handle = window.setTimeout(() => {
      void load(agentType, false)
    }, 250)
    return () => window.clearTimeout(handle)
  }, [agentType, load])

  if (loading) {
    return (
      <div className="flex items-center gap-2 text-xs text-muted-foreground">
        <Loader2 className="size-3.5 animate-spin" aria-hidden="true" />
        {t("probing")}
      </div>
    )
  }
  if (error) {
    return (
      <div className="flex flex-col items-start gap-2">
        <p className="text-xs text-destructive">{error}</p>
        <Button
          size="sm"
          variant="outline"
          onClick={() => void load(agentType, true)}
        >
          {t("retry")}
        </Button>
      </div>
    )
  }
  if (!snapshot) return null

  const hasModes = !!snapshot.modes && snapshot.modes.available_modes.length > 0
  const hasOptions = snapshot.config_options.length > 0
  if (!hasModes && !hasOptions) {
    return <p className="text-xs text-muted-foreground">{t("configNone")}</p>
  }
  // Mirror the composer: when an agent exposes both modes AND config options,
  // hide the standalone mode row (some agents surface mode as a config option).
  const showMode = hasModes && !hasOptions

  return (
    <div className="flex flex-col gap-2.5 rounded-lg border border-border bg-card/40 p-3">
      {showMode && snapshot.modes ? (
        <FlatSelect
          label={t("mode")}
          value={modeId}
          inheritLabel={t("inherit")}
          onChange={onModeChange}
          items={snapshot.modes.available_modes.map((m) => ({
            value: m.id,
            name: m.name,
          }))}
        />
      ) : null}
      {snapshot.config_options.map((option) => (
        <ConfigOptionRow
          key={option.id}
          option={option}
          value={configValues[option.id] ?? null}
          inheritLabel={t("inherit")}
          onChange={(v) => onConfigChange(option.id, v)}
        />
      ))}
    </div>
  )
}

function FlatSelect({
  label,
  value,
  inheritLabel,
  onChange,
  items,
}: {
  label: string
  value: string | null
  inheritLabel: string
  onChange: (v: string | null) => void
  items: Array<{ value: string; name: string }>
}) {
  return (
    <div className="flex items-center justify-between gap-3">
      <label className="min-w-0 truncate text-sm">{label}</label>
      <Select
        value={value ?? DEFAULT_SENTINEL}
        onValueChange={(v) => onChange(v === DEFAULT_SENTINEL ? null : v)}
      >
        <SelectTrigger size="sm" className="w-52">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={DEFAULT_SENTINEL}>{inheritLabel}</SelectItem>
          {items.map((it) => (
            <SelectItem key={it.value} value={it.value}>
              {it.name}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  )
}

function ConfigOptionRow({
  option,
  value,
  inheritLabel,
  onChange,
}: {
  option: SessionConfigOptionInfo
  value: string | null
  inheritLabel: string
  onChange: (v: string | null) => void
}) {
  if (option.kind.type !== "select") return null
  const groups = option.kind.groups
  return (
    <div className="flex items-center justify-between gap-3">
      <label className="min-w-0 truncate text-sm">{option.name}</label>
      <Select
        value={value ?? DEFAULT_SENTINEL}
        onValueChange={(v) => onChange(v === DEFAULT_SENTINEL ? null : v)}
      >
        <SelectTrigger size="sm" className="w-52">
          <SelectValue />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value={DEFAULT_SENTINEL}>{inheritLabel}</SelectItem>
          {groups.length > 0
            ? groups.map((g) => (
                <SelectGroup key={g.group}>
                  <SelectLabel>{g.name}</SelectLabel>
                  {g.options.map((it) => (
                    <SelectItem key={`${g.group}-${it.value}`} value={it.value}>
                      {it.name}
                    </SelectItem>
                  ))}
                </SelectGroup>
              ))
            : option.kind.options.map((it) => (
                <SelectItem key={it.value} value={it.value}>
                  {it.name}
                </SelectItem>
              ))}
        </SelectContent>
      </Select>
    </div>
  )
}
