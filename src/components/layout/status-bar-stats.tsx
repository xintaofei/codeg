"use client"

import { useMemo } from "react"
import { BarChart3, MonitorCloud } from "lucide-react"
import { useTranslations } from "next-intl"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { AGENT_LABELS } from "@/lib/types"
import { AgentIcon } from "@/components/agent-icon"
import { useRemoteConnection } from "@/contexts/remote-connection-context"
import {
  Popover,
  PopoverContent,
  PopoverTrigger,
} from "@/components/ui/popover"
import {
  Tooltip,
  TooltipContent,
  TooltipProvider,
  TooltipTrigger,
} from "@/components/ui/tooltip"

export function StatusBarStats() {
  const t = useTranslations("Folder.statusBar.stats")
  const stats = useAppWorkspaceStore((s) => s.stats)
  // Non-null only in a remote-desktop window (a Tauri client bound to a remote
  // codeg-server); local windows have no RemoteConnection in context.
  const remoteConnection = useRemoteConnection()?.connection ?? null

  const activeAgents = useMemo(
    () => stats?.by_agent.filter((a) => a.conversation_count > 0) ?? [],
    [stats]
  )

  if (!remoteConnection && !stats) return null

  return (
    <div className="flex items-center gap-3">
      {remoteConnection && (
        <TooltipProvider>
          <Tooltip>
            <TooltipTrigger asChild>
              <span className="flex max-w-40 items-center gap-1.5">
                <MonitorCloud className="h-3 w-3 shrink-0" />
                <span className="truncate">{remoteConnection.name}</span>
              </span>
            </TooltipTrigger>
            <TooltipContent side="top" align="start">
              <p className="font-medium">{remoteConnection.name}</p>
              <p className="text-background/70">{remoteConnection.base_url}</p>
            </TooltipContent>
          </Tooltip>
        </TooltipProvider>
      )}
      {stats && (
        <Popover>
          <PopoverTrigger asChild>
            <button className="flex items-center gap-1.5 hover:text-foreground transition-colors">
              <BarChart3 className="h-3 w-3" />
              <span>
                {t("conversations", { count: stats.total_conversations })}
              </span>
            </button>
          </PopoverTrigger>
          <PopoverContent side="top" align="start" className="w-64 p-3">
            <div className="text-xs font-medium mb-2">
              {t("summary", {
                conversations: stats.total_conversations,
                messages: stats.total_messages,
              })}
            </div>
            <div className="space-y-1.5">
              {activeAgents.map((a) => (
                <div
                  key={a.agent_type}
                  className="flex items-center gap-2 text-xs"
                >
                  <AgentIcon agentType={a.agent_type} className="w-3.5 h-3.5" />
                  <span className="text-muted-foreground">
                    {AGENT_LABELS[a.agent_type]}
                  </span>
                  <span className="ml-auto text-muted-foreground">
                    {a.conversation_count}
                  </span>
                </div>
              ))}
            </div>
          </PopoverContent>
        </Popover>
      )}
    </div>
  )
}
