import { memo, useMemo, useState, type ReactNode } from "react"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import type { AgentToolCall } from "@/lib/types"
import { tryParseJson, extractJsonField } from "./content-parts-renderer"
import { MessageResponse } from "@/components/ai-elements/message"
import { Shimmer } from "@/components/ai-elements/shimmer"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { ScrollArea } from "@/components/ui/scroll-area"
import { cn } from "@/lib/utils"
import { ChevronRightIcon, Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"

function formatDuration(ms: number): string {
  if (ms < 1000) return `${ms}ms`
  const sec = ms / 1000
  if (sec < 60) return `${sec.toFixed(1)}s`
  return `${(sec / 60).toFixed(1)}m`
}

/** Convert AgentToolCall[] to AdaptedContentPart[] for reuse with ToolCallPart */
function adaptToolCalls(
  calls: AgentToolCall[],
  parentId: string
): AdaptedContentPart[] {
  return calls.map(
    (call, i): Extract<AdaptedContentPart, { type: "tool-call" }> => ({
      type: "tool-call",
      toolCallId: `${parentId}-sub-${i}`,
      toolName: call.tool_name,
      input: call.input_preview ?? null,
      state: call.is_error ? "output-error" : "output-available",
      output: call.output_preview ?? null,
      errorText: call.is_error ? (call.output_preview ?? undefined) : undefined,
    })
  )
}

// ── main component ────────────────────────────────────────────────────

export const AgentToolCallPart = memo(function AgentToolCallPart({
  part,
  renderToolCall,
}: {
  part: Extract<AdaptedContentPart, { type: "tool-call" }>
  /** Render a single tool-call part — injected by the parent to avoid
   *  circular imports (content-parts-renderer → agent-tool-call → renderer). */
  renderToolCall: (
    part: Extract<AdaptedContentPart, { type: "tool-call" }>,
    key: string
  ) => ReactNode
}) {
  const t = useTranslations("Folder.chat.contentParts")
  const tTool = useTranslations("Folder.chat.tool")

  const isRunning =
    part.state === "input-available" || part.state === "input-streaming"
  const isError = part.state === "output-error"

  const [bodyOpen, setBodyOpen] = useState(isError)
  const [promptOpen, setPromptOpen] = useState(false)

  // Auto-collapse once when the agent transitions from running to completed
  // (non-error) — only matters when the user manually expanded during streaming.
  // Render-phase `setState` driven by tracked previous state is the canonical
  // React pattern for "respond to a prop transition" — see
  // https://react.dev/reference/react/useState#storing-information-from-previous-renders.
  // The transition fires at most once per tool call.
  const [prevIsRunning, setPrevIsRunning] = useState(isRunning)
  if (prevIsRunning !== isRunning) {
    setPrevIsRunning(isRunning)
    if (prevIsRunning && !isRunning && !isError) {
      setBodyOpen(false)
    }
  }

  const parsed = useMemo(
    () => (part.input ? tryParseJson(part.input) : null),
    [part.input]
  )

  const subagentType = useMemo(
    () =>
      (parsed?.subagent_type as string | undefined) ??
      // Codex's live `spawn_agent` payload labels the agent with `agent_type`
      // instead of `subagent_type` (the historical parser already maps it
      // across). Read both so the prefix shows during streaming too.
      (parsed?.agent_type as string | undefined) ??
      (part.input ? extractJsonField(part.input, "subagent_type") : null) ??
      (part.input ? extractJsonField(part.input, "agent_type") : null),
    [parsed, part.input]
  )

  const description = useMemo(
    () =>
      (parsed?.description as string | undefined) ??
      (part.input ? extractJsonField(part.input, "description") : null),
    [parsed, part.input]
  )

  const prompt = useMemo(
    () =>
      (parsed?.prompt as string | undefined) ??
      (part.input ? extractJsonField(part.input, "prompt") : null),
    [parsed, part.input]
  )

  const model = useMemo(
    () =>
      (parsed?.model as string | undefined) ??
      (part.input ? extractJsonField(part.input, "model") : null),
    [parsed, part.input]
  )

  const title = useMemo(() => {
    if (subagentType) {
      return description ? `${subagentType}: ${description}` : subagentType
    }
    // The sub-agent type hasn't streamed in yet. Prefer the description if it
    // has already arrived, and only fall back to the "starting…" placeholder
    // when there's genuinely nothing to show — never prepend it to a title
    // that already carries real content.
    return description || t("agentFallbackTitle")
  }, [subagentType, description, t])

  const statusLabel =
    part.state === "input-available"
      ? tTool("status.inputAvailable")
      : part.state === "input-streaming"
        ? tTool("status.inputStreaming")
        : part.state === "output-available"
          ? tTool("status.outputAvailable")
          : tTool("status.outputError")

  const agentStats = part.agentStats ?? null
  const adaptedToolCalls = useMemo(
    () => adaptToolCalls(agentStats?.tool_calls ?? [], part.toolCallId),
    [agentStats?.tool_calls, part.toolCallId]
  )

  const durationSuffix = useMemo(() => {
    if (!agentStats?.total_duration_ms) return null
    return formatDuration(agentStats.total_duration_ms)
  }, [agentStats])

  return (
    <Collapsible open={bodyOpen} onOpenChange={setBodyOpen} className="w-full">
      {/* Pill trigger — matches ToolGroupPart structure with themed emphasis. */}
      <CollapsibleTrigger
        className={cn(
          "group inline-flex max-w-full items-center gap-1.5 rounded-full bg-primary/10 px-3.5 py-2 text-xs font-medium text-foreground transition-colors hover:bg-primary/15",
          isError && "text-destructive"
        )}
        aria-label={statusLabel}
      >
        <ChevronRightIcon
          aria-hidden="true"
          className={cn(
            "size-3 shrink-0 opacity-60 transition-transform",
            bodyOpen && "rotate-90"
          )}
        />
        <span className="min-w-0 truncate">
          {isRunning ? (
            <Shimmer as="span" duration={1} shineColor="var(--primary)">
              {title}
            </Shimmer>
          ) : (
            title
          )}
        </span>
        {durationSuffix && (
          <span className="shrink-0 text-muted-foreground/60">
            {durationSuffix}
          </span>
        )}
      </CollapsibleTrigger>

      {/* Body — sits below the pill. Internal sections retain their own affordances. */}
      <CollapsibleContent
        className={cn(
          "w-full outline-none",
          "data-[state=open]:animate-in data-[state=closed]:animate-out",
          "data-[state=closed]:fade-out-0 data-[state=open]:fade-in-0",
          "data-[state=closed]:slide-out-to-top-1 data-[state=open]:slide-in-from-top-1"
        )}
      >
        <div className="mt-3 w-full overflow-hidden rounded-md border border-border/60">
          <ScrollArea className="max-h-72">
            <div className="space-y-3 px-3.5 py-2">
              {/* Model summary */}
              {model && (
                <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
                  <span>
                    {t("agentModelLabel")}:{" "}
                    <span className="font-mono">{model}</span>
                  </span>
                </div>
              )}

              {/* Collapsible prompt */}
              {prompt && (
                <Collapsible open={promptOpen} onOpenChange={setPromptOpen}>
                  <CollapsibleTrigger className="flex items-center gap-1 text-xs font-medium text-muted-foreground hover:text-foreground transition-colors">
                    <ChevronRightIcon
                      aria-hidden="true"
                      className={cn(
                        "size-3.5 transition-transform",
                        promptOpen && "rotate-90"
                      )}
                    />
                    {t("agentPromptLabel")}
                  </CollapsibleTrigger>
                  <CollapsibleContent>
                    <div className="mt-2 rounded-md bg-muted/50 p-3 text-xs text-muted-foreground prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
                      <MessageResponse>{prompt}</MessageResponse>
                    </div>
                  </CollapsibleContent>
                </Collapsible>
              )}

              {/* Subagent tool calls — rendered with the same ToolCallPart
              as the outer conversation for consistent appearance */}
              {adaptedToolCalls.length > 0 && (
                <div className="space-y-2">
                  {adaptedToolCalls.map((tc, i) =>
                    renderToolCall(
                      tc as Extract<AdaptedContentPart, { type: "tool-call" }>,
                      `subagent-tc-${i}`
                    )
                  )}
                </div>
              )}

              {/* Running indicator */}
              {isRunning && !part.output && (
                <div className="flex items-center gap-2">
                  <Loader2 className="size-3.5 animate-spin text-muted-foreground" />
                  <Shimmer
                    className="text-sm"
                    duration={1}
                    shineColor="var(--primary)"
                  >
                    {t("agentRunning")}
                  </Shimmer>
                </div>
              )}

              {/* Error output */}
              {isError && part.errorText && (
                <div className="rounded-md bg-destructive/10 p-3">
                  <pre className="whitespace-pre-wrap break-words text-xs text-destructive">
                    {part.errorText}
                  </pre>
                </div>
              )}

              {/* Final output */}
              {part.output && !isError && (
                <div className="text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
                  <MessageResponse>{part.output}</MessageResponse>
                </div>
              )}
            </div>
          </ScrollArea>
        </div>
      </CollapsibleContent>
    </Collapsible>
  )
})
