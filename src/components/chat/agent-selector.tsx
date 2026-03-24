"use client"

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { acpListAgents } from "@/lib/tauri"
import { disposeTauriListener } from "@/lib/tauri-listener"
import type { AgentType, AcpAgentInfo } from "@/lib/types"
import { AGENT_LABELS } from "@/lib/types"
import { AgentIcon } from "@/components/agent-icon"
import { cn } from "@/lib/utils"

const ACP_AGENTS_UPDATED_EVENT = "app://acp-agents-updated"

interface AgentSelectorProps {
  defaultAgentType?: AgentType
  onSelect: (agentType: AgentType) => void
  onAgentsLoaded?: (agents: AcpAgentInfo[]) => void
  onOpenAgentsSettings?: () => void
  disabled?: boolean
}

export function AgentSelector({
  defaultAgentType,
  onSelect,
  onAgentsLoaded,
  onOpenAgentsSettings,
  disabled = false,
}: AgentSelectorProps) {
  const t = useTranslations("Folder.chat.agentSelector")
  const [agents, setAgents] = useState<AcpAgentInfo[]>([])
  const [selected, setSelected] = useState<AgentType | null>(
    defaultAgentType ?? null
  )
  const selectedRef = useRef(selected)
  const onSelectRef = useRef(onSelect)
  const onAgentsLoadedRef = useRef(onAgentsLoaded)

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
    onAgentsLoadedRef.current = onAgentsLoaded
  }, [onAgentsLoaded])

  useEffect(() => {
    let cancelled = false
    let latestRequestId = 0

    const reloadAgents = async () => {
      const requestId = latestRequestId + 1
      latestRequestId = requestId
      try {
        const list = await acpListAgents()
        if (cancelled || requestId !== latestRequestId) return
        const sorted = [...list].sort(
          (a, b) => a.sort_order - b.sort_order || a.name.localeCompare(b.name)
        )
        const visible = sorted.filter((a) => a.enabled)
        setAgents(visible)
        onAgentsLoadedRef.current?.(visible)
        // Keep current selection if still available; otherwise pick first.
        const preferred = defaultAgentType ?? selectedRef.current
        const found = preferred
          ? visible.find((a) => a.agent_type === preferred && a.available)
          : null
        if (found) {
          setSelected(found.agent_type)
          selectedRef.current = found.agent_type
        } else {
          const first = visible.find((a) => a.available)
          if (first) {
            setSelected(first.agent_type)
            selectedRef.current = first.agent_type
            onSelectRef.current(first.agent_type)
          }
        }
      } catch {
        if (!cancelled && requestId === latestRequestId) {
          setAgents([])
          onAgentsLoadedRef.current?.([])
        }
      }
    }

    void reloadAgents()
    const onWindowFocus = () => {
      void reloadAgents()
    }
    window.addEventListener("focus", onWindowFocus)

    let unlisten: (() => void) | null = null
    void import("@tauri-apps/api/event")
      .then(({ listen }) =>
        listen(ACP_AGENTS_UPDATED_EVENT, () => {
          void reloadAgents()
        })
      )
      .then((dispose) => {
        if (cancelled) {
          disposeTauriListener(dispose, "AgentSelector.agentsUpdated")
          return
        }
        unlisten = dispose
      })
      .catch(() => {
        // Ignore when non-tauri runtime.
      })

    return () => {
      cancelled = true
      window.removeEventListener("focus", onWindowFocus)
      disposeTauriListener(unlisten, "AgentSelector.agentsUpdated")
    }
  }, [defaultAgentType])

  const handleSelect = (agentType: AgentType) => {
    setSelected(agentType)
    selectedRef.current = agentType
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
      className="relative inline-flex items-center self-center rounded-full bg-muted/50 p-1 border border-border/50"
    >
      {/* Sliding droplet indicator */}
      {indicator && (
        <div
          className="absolute top-1 bottom-1 rounded-full bg-background shadow-sm ring-1 ring-border/50 transition-all duration-300 ease-[cubic-bezier(0.4,0,0.2,1)]"
          style={{
            left: indicator.left,
            width: indicator.width,
          }}
        />
      )}
      {agents.map((agent) => {
        const isSelected = selected === agent.agent_type
        return (
          <button
            key={agent.agent_type}
            ref={setItemRef(agent.agent_type)}
            title={!isSelected ? AGENT_LABELS[agent.agent_type] : undefined}
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
            <AgentIcon
              agentType={agent.agent_type}
              className="w-4 h-4 shrink-0"
            />
            <span
              className={cn(
                "overflow-hidden whitespace-nowrap transition-all duration-300",
                isSelected
                  ? "max-w-[80px] opacity-100"
                  : "max-w-0 opacity-0"
              )}
            >
              {AGENT_LABELS[agent.agent_type]}
            </span>
          </button>
        )
      })}
    </div>
  )
}
