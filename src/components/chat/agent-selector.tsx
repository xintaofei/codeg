"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { useAcpAgents } from "@/hooks/use-acp-agents"
import type { AgentType, AcpAgentInfo } from "@/lib/types"
import { AGENT_LABELS } from "@/lib/types"
import { AgentIcon } from "@/components/agent-icon"
import { cn } from "@/lib/utils"

interface AgentSelectorProps {
  defaultAgentType?: AgentType
  /** Fires on user click. The caller should treat this as confirmation. */
  onSelect: (agentType: AgentType) => void
  /**
   * Fires when `defaultAgentType` is missing/unavailable and the selector
   * had to pick a substitute on its own. Distinct from `onSelect` so the
   * caller can avoid promoting a system pick to a confirmed user choice
   * (which would otherwise mask a stale-default correction upstream).
   * When omitted, falls back to `onSelect` for backwards compatibility.
   */
  onFallback?: (agentType: AgentType) => void
  onAgentsLoaded?: (agents: AcpAgentInfo[]) => void
  onOpenAgentsSettings?: () => void
  disabled?: boolean
}

export function AgentSelector({
  defaultAgentType,
  onSelect,
  onFallback,
  onAgentsLoaded,
  onOpenAgentsSettings,
  disabled = false,
}: AgentSelectorProps) {
  const t = useTranslations("Folder.chat.agentSelector")
  const { agents: rawAgents } = useAcpAgents()
  const agents = useMemo<AcpAgentInfo[]>(
    () => rawAgents.filter((a) => a.enabled),
    [rawAgents]
  )
  const onSelectRef = useRef(onSelect)
  const onFallbackRef = useRef(onFallback)
  const onAgentsLoadedRef = useRef(onAgentsLoaded)

  // Effective selection. Priority: prop default (when still available) →
  // first available. Derived so we don't have to call setState inside an
  // effect. Click handling lives on the parent — `handleSelect` just
  // forwards via `onSelect`, which patches `defaultAgentType` upstream.
  const selected = useMemo<AgentType | null>(() => {
    const found = defaultAgentType
      ? agents.find((a) => a.agent_type === defaultAgentType && a.available)
      : null
    if (found) return found.agent_type
    const first = agents.find((a) => a.available)
    return first?.agent_type ?? null
  }, [agents, defaultAgentType])

  // Sliding indicator state
  const containerRef = useRef<HTMLDivElement>(null)
  const itemRefs = useRef<Map<AgentType, HTMLButtonElement>>(new Map())
  const [indicator, setIndicator] = useState<{
    left: number
    width: number
  } | null>(null)

  // Use ResizeObserver to track button size changes during CSS transitions
  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    const measure = () => {
      if (!selected) {
        setIndicator(null)
        return
      }
      const btn = itemRefs.current.get(selected)
      if (!btn || !container) {
        setIndicator(null)
        return
      }
      const containerRect = container.getBoundingClientRect()
      const btnRect = btn.getBoundingClientRect()
      setIndicator({
        left: btnRect.left - containerRect.left,
        width: btnRect.width,
      })
    }

    const ro = new ResizeObserver(() => {
      measure()
    })

    // Observe all button elements so indicator updates as they resize
    for (const btn of itemRefs.current.values()) {
      ro.observe(btn)
    }
    ro.observe(container)

    // Initial measurement
    measure()

    const onResize = () => measure()
    window.addEventListener("resize", onResize)

    return () => {
      ro.disconnect()
      window.removeEventListener("resize", onResize)
    }
  }, [selected, agents])

  useEffect(() => {
    onSelectRef.current = onSelect
  }, [onSelect])

  useEffect(() => {
    onFallbackRef.current = onFallback
  }, [onFallback])

  useEffect(() => {
    onAgentsLoadedRef.current = onAgentsLoaded
  }, [onAgentsLoaded])

  // Notify parent when the agent list changes, and emit a *fallback* event
  // (not onSelect) when the requested preferred agent is unavailable and
  // we had to pick a substitute. Splitting the channel matters: the caller
  // treats `onSelect` as a confirmed user choice and clears any "this is a
  // provisional default" flag upstream — if the auto-fallback came through
  // the same path, a hydrated draft whose old agent is now disabled would
  // be silently locked onto sortedTypes[0] before TabProvider's correction
  // effect has a chance to apply the folder's saved default. Callers that
  // don't supply `onFallback` get the legacy behavior (fallback as
  // onSelect) so this prop stays optional.
  useEffect(() => {
    onAgentsLoadedRef.current?.(agents)
    const found = defaultAgentType
      ? agents.find((a) => a.agent_type === defaultAgentType && a.available)
      : null
    if (found) return
    const first = agents.find((a) => a.available)
    if (!first) return
    const fallback = onFallbackRef.current
    if (fallback) {
      fallback(first.agent_type)
    } else {
      onSelectRef.current(first.agent_type)
    }
  }, [agents, defaultAgentType])

  const handleSelect = (agentType: AgentType) => {
    onSelect(agentType)
  }

  const setItemRef = useCallback(
    (agentType: AgentType) => (el: HTMLButtonElement | null) => {
      if (el) {
        itemRefs.current.set(agentType, el)
      } else {
        itemRefs.current.delete(agentType)
      }
    },
    []
  )

  if (agents.length === 0) {
    return (
      <div className="rounded-lg border border-dashed bg-muted/30 px-4 py-3 text-center text-sm text-muted-foreground">
        <div>{t("noEnabledAgents")}</div>
        {onOpenAgentsSettings ? (
          <button
            type="button"
            onClick={onOpenAgentsSettings}
            className="mt-2 inline-flex items-center rounded-md border px-2 py-1 text-xs text-foreground transition-colors hover:bg-accent cursor-pointer"
          >
            {t("openAgentsSettings")}
          </button>
        ) : null}
      </div>
    )
  }

  return (
    <div
      ref={containerRef}
      className="relative inline-flex items-center self-center rounded-full bg-muted/50 p-0.5 border border-border/50"
    >
      {/* Sliding droplet indicator */}
      {indicator && (
        <div
          className="absolute top-0.5 bottom-0.5 rounded-full bg-background shadow-sm ring-1 ring-border/50 transition-all duration-300 ease-[cubic-bezier(0.4,0,0.2,1)]"
          style={{
            left: indicator.left,
            width: indicator.width,
          }}
        />
      )}
      {agents.map((agent) => {
        const isSelected = selected === agent.agent_type
        // Enabled + platform-available, but the CLI/SDK isn't installed. Kept
        // clickable (selecting it surfaces a persistent install prompt in the
        // composer) but flagged with a marker + tooltip so it reads as "needs
        // install" instead of looking identical to a ready agent.
        const notInstalled = agent.available && !agent.installed_version
        const label = AGENT_LABELS[agent.agent_type]
        return (
          <button
            key={agent.agent_type}
            ref={setItemRef(agent.agent_type)}
            title={
              notInstalled
                ? `${label} · ${t("notInstalled")}`
                : !isSelected
                  ? label
                  : undefined
            }
            disabled={disabled || !agent.available}
            onClick={() => handleSelect(agent.agent_type)}
            className={cn(
              "relative z-10 inline-flex items-center justify-center gap-1.5 rounded-full text-xs font-medium transition-all duration-300",
              isSelected ? "px-3 py-2" : "px-2 py-2",
              disabled || !agent.available
                ? "cursor-not-allowed opacity-40"
                : "cursor-pointer",
              isSelected
                ? "text-foreground"
                : "text-muted-foreground hover:text-foreground/70"
            )}
          >
            <span className="relative shrink-0">
              <AgentIcon agentType={agent.agent_type} className="h-4 w-4" />
              {notInstalled ? (
                <span
                  aria-hidden
                  className="absolute -top-0.5 -right-0.5 h-1.5 w-1.5 rounded-full bg-amber-500 ring-1 ring-background"
                />
              ) : null}
            </span>
            <span
              className={cn(
                "grid transition-[grid-template-columns] duration-300",
                isSelected ? "grid-cols-[1fr]" : "grid-cols-[0fr]"
              )}
            >
              <span
                className={cn(
                  "min-w-0 overflow-hidden whitespace-nowrap transition-opacity duration-300",
                  isSelected ? "opacity-100" : "opacity-0"
                )}
              >
                {label}
              </span>
            </span>
          </button>
        )
      })}
    </div>
  )
}
