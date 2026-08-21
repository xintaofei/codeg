"use client"

/**
 * Inline card for the codeg-mcp *workbench* companion tools — `get_session_info`,
 * `task_progress`, `task_complete`, `create_automation` and `create_work_task`.
 *
 * One collapsed line framed around what the agent was actually doing ("读取会话
 * #2122", "进度：tests passing", "任务完成 · 成功"), with a status badge and an
 * inline expansion for the result text. Deliberately the same shape and visual
 * language as `DelegationStatusCard` / `DelegationStatusRow` so every codeg-mcp
 * companion reads as one family in the transcript instead of these five landing
 * on the generic tool shell as a raw `{"message": "…"}` argument dump.
 *
 * Parsing lives in `@/lib/codeg-mcp-tool` — it handles the per-host argument
 * wrapping and result envelopes, so this component is presentation-only.
 */

import { useId, useMemo, useState } from "react"
import { useTranslations } from "next-intl"
import {
  BookOpen,
  CalendarClock,
  ChevronDown,
  ChevronRight,
  CircleCheckBig,
  ClipboardList,
  Signpost,
} from "lucide-react"

import { cn } from "@/lib/utils"
import {
  parseCodegMcpToolCall,
  type CodegMcpWorkbenchTool,
} from "@/lib/codeg-mcp-tool"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"
import { MessageResponse } from "@/components/ai-elements/message"
import { Shimmer } from "@/components/ai-elements/shimmer"
import { StatusBadge } from "@/components/message/delegation-status-badge"

interface Props {
  /** Canonical tool name — selects the label, icon and argument to state. */
  tool: CodegMcpWorkbenchTool
  /** Raw JSON arguments the agent sent to the tool. */
  input?: string | null
  output?: string | null
  errorText?: string | null
  state?: ToolCallState
}

const ICONS: Record<CodegMcpWorkbenchTool, typeof BookOpen> = {
  get_session_info: BookOpen,
  task_progress: Signpost,
  task_complete: CircleCheckBig,
  create_automation: CalendarClock,
  create_work_task: ClipboardList,
}

export function CodegMcpToolCard({
  tool,
  input,
  output,
  errorText,
  state,
}: Props) {
  const t = useTranslations("Folder.chat.codegMcpTool")
  const [expanded, setExpanded] = useState(false)
  const panelId = useId()

  const model = useMemo(
    () => parseCodegMcpToolCall({ tool, input, output, errorText, state }),
    [tool, input, output, errorText, state]
  )

  const label = useMemo(() => {
    switch (tool) {
      case "get_session_info":
        return model.detail
          ? t("getSessionInfo", { session: `#${model.detail}` })
          : t("getSessionInfoNoId")
      case "task_progress":
        return model.detail
          ? t("taskProgress", { message: model.detail })
          : t("taskProgressNoMessage")
      case "task_complete": {
        // The verdict is the headline; the summary rides along when the agent
        // supplied one (it is optional in the schema).
        const verdict = model.verdict
          ? t(`verdict.${model.verdict}`)
          : t("verdict.unknown")
        return model.detail
          ? t("taskCompleteWithSummary", { verdict, summary: model.detail })
          : t("taskComplete", { verdict })
      }
      case "create_automation":
        return model.detail
          ? t("createAutomation", { name: model.detail })
          : t("createAutomationNoName")
      case "create_work_task":
        return model.detail
          ? t("createWorkTask", { title: model.detail })
          : t("createWorkTaskNoTitle")
    }
  }, [t, tool, model])

  const Icon = ICONS[tool]
  const isError = model.status === "err"
  const isRunning = model.status === "running"
  const resultText = model.resultText?.trim() ?? ""
  const prompt = model.prompt?.trim() ?? ""
  // A still-running call has nothing to reveal yet; so does one whose result was
  // a bare ack ("Recorded.") already implied by the badge — but we can't tell
  // those apart without guessing at wording, so any result text is expandable.
  // An authoring call is expandable from frame 1: its prompt is worth auditing
  // and arrives with the arguments, long before any result.
  const expandable = resultText !== "" || prompt !== ""

  const row = (
    <>
      <Icon
        className={cn(
          "h-3.5 w-3.5 shrink-0",
          isError ? "text-destructive" : "text-muted-foreground"
        )}
      />
      {/* `title` restores the full text on hover: a long progress message or
          task summary is CSS-truncated here and the expandable panel below
          carries the RESULT, not the arguments — so this is the only place it
          can be read back. */}
      <span
        title={label}
        className="min-w-0 flex-1 truncate text-xs font-medium text-foreground"
      >
        {isRunning ? (
          <Shimmer as="span" duration={1} shineColor="var(--primary)">
            {label}
          </Shimmer>
        ) : (
          label
        )}
      </span>
      <StatusBadge status={model.status} />
      {expandable &&
        (expanded ? (
          <ChevronDown className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        ) : (
          <ChevronRight className="h-3.5 w-3.5 shrink-0 text-muted-foreground" />
        ))}
    </>
  )

  return (
    <div
      data-testid="codeg-mcp-tool-card"
      data-tool={tool}
      className={cn(
        "overflow-hidden rounded-lg border text-xs ws-msg-card",
        isError
          ? "border-destructive/30 bg-destructive/5"
          : "border-border bg-card"
      )}
    >
      {expandable ? (
        <button
          type="button"
          onClick={() => setExpanded((v) => !v)}
          aria-expanded={expanded}
          // Only reference the panel while it is mounted — it is kept out of the
          // collapsed tree so the Markdown renderer isn't paid for unopened rows.
          aria-controls={expanded ? panelId : undefined}
          className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-muted/50"
        >
          {row}
        </button>
      ) : (
        <div className="flex w-full items-center gap-2 px-3 py-2">{row}</div>
      )}
      {expandable && expanded && (
        <div id={panelId} className="border-t border-border">
          {/* The instruction the automation / board task will run. Shown as
              plain pre-wrapped text, NOT Markdown: it is an argument the user
              is auditing, so it must read exactly as it was sent. */}
          {prompt !== "" && (
            <div className="px-3 pt-2">
              <div className="mb-1 text-[10px] font-medium uppercase tracking-wide text-muted-foreground">
                {t("promptLabel")}
              </div>
              <div className="max-h-40 overflow-auto whitespace-pre-wrap break-words rounded-md bg-muted/40 px-2 py-1.5 text-xs text-foreground/90">
                {prompt}
              </div>
            </div>
          )}
          {resultText !== "" && (
            <div className='max-h-80 overflow-auto px-3 pb-2 pt-2 prose prose-sm max-w-none break-words text-xs dark:prose-invert [&_ol]:list-inside [&_ul]:list-inside [&_[data-streamdown="code-block-body"]]:max-h-96 [&_[data-streamdown="code-block-body"]]:overflow-auto'>
              <MessageResponse>{resultText}</MessageResponse>
            </div>
          )}
          {prompt !== "" && resultText === "" && <div className="pb-2" />}
        </div>
      )}
    </div>
  )
}
