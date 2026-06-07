"use client"

import { useTranslations } from "next-intl"
import { AgentIcon } from "@/components/agent-icon"
import { AGENT_LABELS } from "@/lib/types"
import { closePetPanel, focusConversation } from "@/lib/pet/api"
import type { PetSessionEntry } from "@/lib/pet/types"
import {
  sessionStatusKind,
  type PetSessionStatusKind,
} from "@/lib/pet/session-display"
import { cn } from "@/lib/utils"
import { PanelPermissionCard } from "./PanelPermissionCard"

interface SessionRowProps {
  session: PetSessionEntry
}

const STATUS_DOT: Record<PetSessionStatusKind, string> = {
  waiting: "bg-amber-500",
  error: "bg-red-500",
  running: "bg-blue-500",
}

export function SessionRow({ session }: SessionRowProps) {
  const t = useTranslations("Pet")
  const kind = sessionStatusKind(session)

  const jump = () => {
    void focusConversation(
      session.folderId,
      session.conversationId,
      session.agentType
    )
      .then(() => closePetPanel())
      .catch((err) =>
        console.warn("[PetPanel] focus conversation failed:", err)
      )
  }

  const statusLabel =
    kind === "waiting"
      ? t("panel.statusWaiting")
      : kind === "error"
        ? t("panel.statusError")
        : t("panel.statusRunning")

  return (
    <div className="px-2 py-1.5">
      <button
        type="button"
        onClick={jump}
        className={cn(
          "flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left",
          "transition-colors hover:bg-accent focus-visible:bg-accent focus-visible:outline-none"
        )}
      >
        <AgentIcon agentType={session.agentType} className="h-4 w-4 shrink-0" />
        <span className="min-w-0 flex-1 truncate text-sm">
          {session.title || AGENT_LABELS[session.agentType]}
        </span>
        <span className="flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground">
          <span
            className={cn(
              "h-1.5 w-1.5 rounded-full",
              STATUS_DOT[kind],
              kind === "running" && "animate-pulse"
            )}
            aria-hidden
          />
          {statusLabel}
        </span>
      </button>

      {session.pending ? (
        <PanelPermissionCard
          connectionId={session.connectionId}
          permission={session.pending}
        />
      ) : null}
    </div>
  )
}
