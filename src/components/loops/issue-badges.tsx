"use client"

import { useTranslations } from "next-intl"

import { Badge } from "@/components/ui/badge"
import type {
  LoopIssuePriority,
  LoopIssueRoute,
  LoopIssueStatus,
} from "@/lib/types"

type BadgeVariant =
  | "default"
  | "secondary"
  | "destructive"
  | "outline"
  | "ghost"

const STATUS_VARIANT: Record<LoopIssueStatus, BadgeVariant> = {
  pending: "outline",
  running: "default",
  paused: "secondary",
  blocked: "destructive",
  done: "secondary",
  cancelled: "ghost",
}

const PRIORITY_VARIANT: Record<LoopIssuePriority, BadgeVariant> = {
  high: "destructive",
  medium: "secondary",
  low: "outline",
}

export function IssueStatusBadge({ status }: { status: LoopIssueStatus }) {
  const t = useTranslations("Loops.status")
  return <Badge variant={STATUS_VARIANT[status]}>{t(status)}</Badge>
}

export function IssuePriorityBadge({
  priority,
}: {
  priority: LoopIssuePriority
}) {
  const t = useTranslations("Loops.priority")
  return <Badge variant={PRIORITY_VARIANT[priority]}>{t(priority)}</Badge>
}

export function IssueRouteBadge({ route }: { route: LoopIssueRoute }) {
  const t = useTranslations("Loops.route")
  // The undecided route is the default pre-triage state — not worth a chip.
  if (route === "undecided") return null
  return <Badge variant="outline">{t(route)}</Badge>
}
