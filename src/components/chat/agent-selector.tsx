"use client"

import { useEffect, useRef, useState } from "react"
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
    <div className="flex flex-wrap items-center justify-center gap-2">
      {agents.map((agent) => (
        <button
          key={agent.agent_type}
          disabled={disabled || !agent.available}
          onClick={() => handleSelect(agent.agent_type)}
          className={cn(
            "inline-flex items-center gap-1.5 rounded-full px-3 py-1.5 text-xs font-medium transition-colors",
            "border",
            disabled || !agent.available
              ? "cursor-not-allowed opacity-40"
              : "cursor-pointer hover:bg-accent",
            selected === agent.agent_type
              ? "border-primary bg-primary/10 text-primary"
              : "border-border text-muted-foreground"
          )}
        >
          <AgentIcon agentType={agent.agent_type} className="w-3.5 h-3.5" />
          {AGENT_LABELS[agent.agent_type]}
        </button>
      ))}
    </div>
  )
}
