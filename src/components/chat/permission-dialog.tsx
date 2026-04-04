"use client"

import { useMemo } from "react"
import { useTranslations } from "next-intl"
import {
  ShieldAlert,
  Terminal,
  ListTodo,
  Compass,
  FileText,
  Globe,
  Search,
} from "lucide-react"
import { Badge } from "@/components/ui/badge"
import { CodeBlock } from "@/components/ai-elements/code-block"
import { UnifiedDiffPreview } from "@/components/diff/unified-diff-preview"
import { MessageResponse } from "@/components/ai-elements/message"
import { PermissionActions } from "@/components/chat/permission-actions"
import type { PendingPermission } from "@/contexts/acp-connections-context"
import { parsePermissionToolCall } from "@/lib/permission-request"

interface PermissionDialogProps {
  permission: PendingPermission | null
  onRespond: (requestId: string, optionId: string) => void
}

function formatKindLabel(kind: string, fallbackLabel: string): string {
  const normalized = kind.replace(/_/g, " ").trim()
  return normalized.length > 0 ? normalized : fallbackLabel
}

export function PermissionDialog({
  permission,
  onRespond,
}: PermissionDialogProps) {
  const t = useTranslations("Folder.chat.permissionDialog")

  const parsed = useMemo(
    () => parsePermissionToolCall(permission?.tool_call),
    [permission?.tool_call]
  )
  if (!permission) return null

  const hasFileChanges = parsed.fileChanges.length > 0
  const hasPlan =
    parsed.planEntries.length > 0 || Boolean(parsed.planExplanation)
  const hasPlanMarkdown = Boolean(parsed.planMarkdown)
  const hasAllowedPrompts = parsed.allowedPrompts.length > 0
  const hasWeb = Boolean(parsed.url) || Boolean(parsed.query)
  const hasStructured =
    Boolean(parsed.command) ||
    hasFileChanges ||
    hasPlan ||
    hasPlanMarkdown ||
    hasAllowedPrompts ||
    Boolean(parsed.modeTarget) ||
    hasWeb

  return (
    <div className="mx-4 mb-3 overflow-hidden rounded-xl border border-border/70 bg-card/95 p-3 shadow-sm">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0 space-y-1">
          <div className="flex items-center gap-1.5 text-sm font-medium">
            <ShieldAlert className="h-4 w-4 shrink-0 text-amber-500" />
            <span className="truncate">{parsed.title}</span>
          </div>
          <p className="text-xs text-muted-foreground">{t("subtitle")}</p>
        </div>
        <Badge variant="outline" className="shrink-0 text-[10px]">
          {formatKindLabel(parsed.normalizedKind, t("kindFallbackTool"))}
        </Badge>
      </div>

      <div className="mt-3 max-h-[min(36vh,18rem)] space-y-2 overflow-y-auto pr-1">
        {parsed.command && (
          <div className="space-y-1.5 rounded-md border border-border/60 bg-muted/20 p-2">
            <div className="flex items-center gap-1 text-xs text-muted-foreground">
              <Terminal className="h-3.5 w-3.5" />
              <span>{t("command")}</span>
            </div>
            <CodeBlock code={parsed.command} language="bash" />
            {parsed.cwd && (
              <div className="break-all text-xs text-muted-foreground">
                {t("cwd", { cwd: parsed.cwd })}
              </div>
            )}
          </div>
        )}

        {hasFileChanges && parsed.diffPreview && (
          <UnifiedDiffPreview diffText={parsed.diffPreview} />
        )}

        {hasPlan && (
          <div className="space-y-1.5 rounded-md border border-border/60 bg-muted/20 p-2">
            <div className="flex items-center gap-1 text-xs text-muted-foreground">
              <ListTodo className="h-3.5 w-3.5" />
              <span>{t("plan")}</span>
            </div>
            {parsed.planExplanation && (
              <p className="text-xs text-foreground/90">
                {parsed.planExplanation}
              </p>
            )}
            {parsed.planEntries.length > 0 && (
              <div className="space-y-1 rounded-md bg-muted/40 p-2">
                {parsed.planEntries.map((entry, index) => (
                  <div key={`${entry.text}-${index}`} className="text-xs">
                    <span className="text-foreground/90">{entry.text}</span>
                    {entry.status && (
                      <span className="ml-2 text-muted-foreground">
                        ({entry.status})
                      </span>
                    )}
                  </div>
                ))}
              </div>
            )}
          </div>
        )}

        {hasPlanMarkdown && (
          <div className="space-y-1.5 rounded-md border border-border/60 bg-muted/20 p-2">
            <div className="flex items-center gap-1 text-xs text-muted-foreground">
              <FileText className="h-3.5 w-3.5" />
              <span>{t("plan")}</span>
            </div>
            <div className="text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
              <MessageResponse>{parsed.planMarkdown!}</MessageResponse>
            </div>
          </div>
        )}

        {hasAllowedPrompts && (
          <div className="space-y-1.5 rounded-md border border-border/60 bg-muted/20 p-2">
            <div className="flex items-center gap-1 text-xs text-muted-foreground">
              <Terminal className="h-3.5 w-3.5" />
              <span>{t("allowedActions")}</span>
            </div>
            <div className="space-y-1 rounded-md bg-muted/40 p-2">
              {parsed.allowedPrompts.map((item, index) => (
                <div
                  key={`${item.prompt}-${index}`}
                  className="flex items-center gap-2 text-xs"
                >
                  {item.tool && (
                    <Badge variant="outline" className="shrink-0 text-[10px]">
                      {item.tool}
                    </Badge>
                  )}
                  <span className="text-foreground/90">{item.prompt}</span>
                </div>
              ))}
            </div>
          </div>
        )}

        {parsed.modeTarget && (
          <div className="rounded-md border border-border/60 bg-muted/20 p-2 text-xs">
            <div className="flex items-center gap-1 text-muted-foreground">
              <Compass className="h-3.5 w-3.5" />
              <span>{t("targetMode", { mode: parsed.modeTarget })}</span>
            </div>
          </div>
        )}

        {hasWeb && (
          <div className="space-y-1.5 rounded-md border border-border/60 bg-muted/20 p-2">
            {parsed.url && (
              <div className="flex items-center gap-2 text-xs">
                <Globe className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                <span className="break-all font-mono text-foreground/90">
                  {parsed.url}
                </span>
              </div>
            )}
            {parsed.query && (
              <div className="flex items-center gap-2 text-xs">
                <Search className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
                <span className="break-all text-foreground/90">
                  {parsed.query}
                </span>
              </div>
            )}
            {parsed.prompt && (
              <div className="mt-1 text-xs text-muted-foreground">
                <MessageResponse>{parsed.prompt}</MessageResponse>
              </div>
            )}
          </div>
        )}

        {!hasStructured && (
          <pre className="rounded-md border border-border/60 bg-muted/20 p-2 text-xs whitespace-pre-wrap break-all text-foreground/90">
            {parsed.jsonPreview}
          </pre>
        )}
      </div>

      <PermissionActions
        options={permission.options}
        onRespond={(optionId) => onRespond(permission.request_id, optionId)}
      />
    </div>
  )
}
