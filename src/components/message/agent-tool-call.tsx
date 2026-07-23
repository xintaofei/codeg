import { memo, useMemo, useState, type ReactNode } from "react"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import type { AgentToolCall } from "@/lib/types"
import { tryParseJson, extractJsonField } from "./content-parts-renderer"
import { shortAgentId } from "@/lib/collab-tool"
import { MessageResponse } from "@/components/ai-elements/message"
import { Shimmer } from "@/components/ai-elements/shimmer"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/instant-collapsible"
import { cn } from "@/lib/utils"
import { ChevronRightIcon, Clock3, Loader2 } from "lucide-react"
import { useTranslations } from "next-intl"
import { AgentCapsule } from "./agent-capsule"
import {
  isAsyncLaunchAckText,
  parseBackgroundTaskMarker,
} from "@/lib/background-agent"

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

// A parsed JSON field is only usable here if it's a non-empty STRING. Some
// hosts (e.g. CodeBuddy) hand us inputs where `subagent_type` / `description`
// arrive as objects (or empty `{}`); the old `as string` casts let those leak
// straight into the rendered `title`, crashing React with "Objects are not
// valid as a React child". Coerce so a non-string field is treated as absent.
function asText(v: unknown): string | null {
  return typeof v === "string" && v.length > 0 ? v : null
}

interface TaskOutcomeEnvelope {
  durationMs: number | null
  isBackground: boolean
  error: string | null
}

// Cursor's live task completions carry a bare JSON envelope instead of report
// text: success → {durationMs, isBackground}, failure → {error} — and the
// wire status stays "completed" either way. `isTask` gates folding to inputs
// that prove the call is a Cursor task (the `_toolName:"task"` stamp): another
// agent's sub-agent legitimately returning `{"error":...}` text must render
// as-is, not get repainted as a failure. Shape stays exact-keys on top.
function parseTaskOutcomeEnvelope(
  output: string | null | undefined,
  isTask: boolean
): TaskOutcomeEnvelope | null {
  if (!isTask || !output) return null
  const trimmed = output.trim()
  if (!trimmed.startsWith("{")) return null
  let parsed: unknown
  try {
    parsed = JSON.parse(trimmed)
  } catch {
    return null
  }
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) {
    return null
  }
  const obj = parsed as Record<string, unknown>
  const keys = Object.keys(obj)
  if (keys.length === 0) return null
  if (
    keys.every((k) => k === "durationMs" || k === "isBackground") &&
    (!("durationMs" in obj) || typeof obj.durationMs === "number") &&
    (!("isBackground" in obj) || typeof obj.isBackground === "boolean")
  ) {
    const duration = obj.durationMs
    return {
      durationMs:
        typeof duration === "number" && Number.isFinite(duration)
          ? duration
          : null,
      isBackground: obj.isBackground === true,
      error: null,
    }
  }
  if (
    keys.length === 1 &&
    keys[0] === "error" &&
    typeof obj.error === "string" &&
    obj.error.length > 0
  ) {
    return { durationMs: null, isBackground: false, error: obj.error }
  }
  return null
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
  const tBg = useTranslations("Folder.chat.backgroundTasks")

  const isRunning =
    part.state === "input-available" || part.state === "input-streaming"
  const isError = part.state === "output-error"

  const parsed = useMemo(
    () => (part.input ? tryParseJson(part.input) : null),
    [part.input]
  )

  // Background sub-agent lifecycle. Historical/refetched turns carry the
  // parser's structured marker (settled state + summary + result folded from
  // the transcript's task-notification); a live turn still holds the raw wire
  // ack text, shown as "running in background" instead of being dumped. An
  // unsettled marker (null status) deliberately reads "result pending", not
  // "running" — the transcript alone can't prove the task is still alive.
  const backgroundLifecycle = useMemo(
    () => parseBackgroundTaskMarker(part.output),
    [part.output]
  )
  // Cursor task completion envelope — fold into the capsule chrome (duration
  // suffix / error box / background label) instead of dumping raw JSON into
  // the body. Gated on the live input's `_toolName:"task"` identity stamp.
  const taskOutcome = useMemo(
    () => parseTaskOutcomeEnvelope(part.output, parsed?._toolName === "task"),
    [part.output, parsed]
  )
  const outcomeError = taskOutcome?.error ?? null
  const outcomeBackground = taskOutcome?.isBackground === true
  const isLiveBackgroundLaunch =
    backgroundLifecycle === null &&
    part.state === "output-available" &&
    isAsyncLaunchAckText(part.output)
  const backgroundSettled = backgroundLifecycle?.status != null
  const backgroundFailed =
    backgroundSettled && backgroundLifecycle?.status !== "completed"

  const [promptOpen, setPromptOpen] = useState(false)

  const subagentType = useMemo(
    () =>
      asText(parsed?.subagent_type) ??
      // Codex's live `spawn_agent` payload labels the agent with `agent_type`
      // instead of `subagent_type` (the historical parser already maps it
      // across). Read both so the prefix shows during streaming too.
      asText(parsed?.agent_type) ??
      // Cursor's live task payload carries `subagentType` as a protobuf-es
      // oneof object ({case: "generalPurpose", …}); its history parser emits
      // a plain snake_case string, so read the live case here for parity.
      asText(parsed?.subagentType) ??
      asText((parsed?.subagentType as { case?: unknown } | undefined)?.case) ??
      (part.input ? extractJsonField(part.input, "subagent_type") : null) ??
      (part.input ? extractJsonField(part.input, "agent_type") : null),
    [parsed, part.input]
  )

  const description = useMemo(
    () =>
      asText(parsed?.description) ??
      (part.input ? extractJsonField(part.input, "description") : null),
    [parsed, part.input]
  )

  const prompt = useMemo(
    () =>
      asText(parsed?.prompt) ??
      (part.input ? extractJsonField(part.input, "prompt") : null),
    [parsed, part.input]
  )

  const model = useMemo(
    () =>
      asText(parsed?.model) ??
      (part.input ? extractJsonField(part.input, "model") : null),
    [parsed, part.input]
  )

  // codex spawn capsules carry the sub-agent's UUID (`agent_id`); show it in the
  // pill so the execution capsule reads uniformly with the live/wait collab
  // capsules. Other agents (e.g. Claude Task) have no `agent_id` → no badge.
  const agentId = useMemo(
    () =>
      asText(parsed?.agent_id) ??
      (part.input ? extractJsonField(part.input, "agent_id") : null),
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

  const statusLabel = backgroundLifecycle
    ? backgroundFailed
      ? tBg("cardFinishedWithStatus", {
          status: backgroundLifecycle.status ?? "",
        })
      : backgroundSettled
        ? tBg("cardCompleted")
        : tBg("cardLaunchedPending")
    : isLiveBackgroundLaunch
      ? tBg("cardRunning")
      : outcomeError
        ? // Cursor reports a failed task with wire status "completed"; the
          // error envelope is the only failure signal.
          tTool("status.outputError")
        : outcomeBackground
          ? // A background task's completion envelope only acknowledges the
            // launch — the sub-agent is still running.
            tBg("cardRunning")
          : part.state === "input-available"
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
    if (agentStats?.total_duration_ms) {
      return formatDuration(agentStats.total_duration_ms)
    }
    if (taskOutcome?.durationMs != null) {
      return formatDuration(taskOutcome.durationMs)
    }
    return null
  }, [agentStats, taskOutcome])

  return (
    <AgentCapsule
      title={title}
      isRunning={isRunning || isLiveBackgroundLaunch || outcomeBackground}
      isError={isError || backgroundFailed || outcomeError != null}
      rightSuffix={durationSuffix}
      idBadge={agentId ? shortAgentId(agentId) : null}
      statusLabel={statusLabel}
    >
      {/* Model summary */}
      {model && (
        <div className="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
          <span>
            {t("agentModelLabel")}: <span className="font-mono">{model}</span>
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

      {/* Running indicator (in-turn streaming, a live background launch whose
          ack just replaced the stream, or a cursor background-task envelope) */}
      {((isRunning && !part.output) ||
        isLiveBackgroundLaunch ||
        outcomeBackground) && (
        <div className="flex items-center gap-2">
          <Loader2 className="size-3.5 animate-spin text-muted-foreground" />
          <Shimmer className="text-sm" duration={1} shineColor="var(--primary)">
            {isLiveBackgroundLaunch || outcomeBackground
              ? tBg("cardRunning")
              : t("agentRunning")}
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

      {/* Cursor task failure envelope ({error}) — the wire marks the call
          "completed", so this renders where the error styling belongs. */}
      {outcomeError && !isError && (
        <div className="rounded-md bg-destructive/10 p-3">
          <pre className="whitespace-pre-wrap break-words text-xs text-destructive">
            {outcomeError}
          </pre>
        </div>
      )}

      {/* Background lifecycle: settled summary + folded result markdown, or a
          neutral "result pending" line for an unsettled launch. Never dumps
          the marker/ack text. */}
      {backgroundLifecycle && !isError && (
        <div className="space-y-2">
          {backgroundLifecycle.summary && (
            <div className="text-xs text-muted-foreground">
              {backgroundLifecycle.summary}
            </div>
          )}
          {backgroundLifecycle.result ? (
            <div className="text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
              <MessageResponse>{backgroundLifecycle.result}</MessageResponse>
            </div>
          ) : !backgroundSettled ? (
            <div className="flex items-center gap-2 text-xs text-muted-foreground">
              <Clock3 className="size-3.5 shrink-0" />
              {tBg("cardResultPending")}
            </div>
          ) : null}
        </div>
      )}

      {/* Final output. A folded task-outcome envelope renders via the
          capsule chrome above (duration suffix / error box), never as body. */}
      {part.output &&
        !isError &&
        !taskOutcome &&
        !backgroundLifecycle &&
        !isLiveBackgroundLaunch && (
          <div className="text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside">
            <MessageResponse>{part.output}</MessageResponse>
          </div>
        )}
    </AgentCapsule>
  )
})
