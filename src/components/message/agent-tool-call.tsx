import { memo, useMemo, useState, type ReactNode } from "react"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import type { AgentToolCall } from "@/lib/types"
import { SubagentSessionDialog } from "./subagent-session-dialog"
import { useSessionViewerHost } from "./session-viewer-host"
import { shortAgentId } from "@/lib/collab-tool"
import {
  childSessionOfLaunch,
  parseSubAgentLaunchFields,
} from "@/lib/native-subagent-fields"
import { MessageResponse } from "@/components/ai-elements/message"
import { Shimmer } from "@/components/ai-elements/shimmer"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/instant-collapsible"
import { cn } from "@/lib/utils"
import {
  CheckIcon,
  ChevronRightIcon,
  CircleDashed,
  Clock3,
  Loader2,
  OctagonX,
} from "lucide-react"
import { useTranslations } from "next-intl"
import { AgentCapsule } from "./agent-capsule"
import { SubagentSessionButton } from "./subagent-session-button"
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

/** Render bound for the live subagent transcript — the DATA is uncapped;
 *  only the visible tail is limited (entries only split at kind boundaries,
 *  so the count stays small in practice; this is a backstop). */
const AGENT_TRANSCRIPT_RENDER_TAIL = 20

interface GrokSubagentProgress {
  durationMs: number | null
  turnCount: number | null
  toolCallCount: number | null
  contextUsagePct: number | null
}

/**
 * Grok's live sub-agent progress, forwarded by the backend as
 * `meta.grokSubagentProgress` on the launching Agent tool call
 * (`connection.rs::map_grok_subagent_notification`, from grok 0.2.11x's
 * `subagent_progress` ext notification). Grok never streams a child's chunks
 * or tool calls over ACP, so this ticker is the only live signal of what the
 * child is doing. `null` for any other meta shape.
 */
function parseGrokSubagentProgress(
  meta: Record<string, unknown> | null | undefined
): GrokSubagentProgress | null {
  if (!meta || typeof meta !== "object") return null
  const raw = (meta as Record<string, unknown>).grokSubagentProgress
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null
  const obj = raw as Record<string, unknown>
  const num = (v: unknown): number | null =>
    typeof v === "number" && Number.isFinite(v) ? v : null
  const progress: GrokSubagentProgress = {
    durationMs: num(obj.durationMs),
    turnCount: num(obj.turnCount),
    toolCallCount: num(obj.toolCallCount),
    contextUsagePct: num(obj.contextUsagePct),
  }
  return progress.durationMs != null ||
    progress.turnCount != null ||
    progress.toolCallCount != null ||
    progress.contextUsagePct != null
    ? progress
    : null
}

/**
 * How the codex sub-agent itself ended, as a pill chip.
 *
 * Separate from the capsule's own status chrome, and deliberately so: the card
 * settles when codex acknowledges the LAUNCH, so its "completed" says nothing
 * about the child, which may still be working. codex reports the real outcome
 * later as `SubAgentActivity{kind}` — live via `settle_codex_subagent_launch`,
 * on reload via the rollout parser — and both write it to the same key.
 *
 * `null` (no outcome heard yet) is a state in its own right, not an absence:
 * the child's fate is genuinely unknown, and saying so is the whole reason this
 * chip exists. That case keeps the full explanation in the body, since a
 * two-word chip cannot carry a caveat.
 */
function CodexSubagentStateBadge({ state }: { state: string | null }) {
  const t = useTranslations("Folder.chat.contentParts")
  const [Icon, label, tone] =
    state === "completed"
      ? ([CheckIcon, t("agentSubagentDone"), "text-green-600"] as const)
      : state === "interrupted"
        ? ([OctagonX, t("agentSubagentInterrupted"), "text-amber-500"] as const)
        : ([
            CircleDashed,
            t("agentSubagentUnknown"),
            "text-muted-foreground/70",
          ] as const)

  return (
    <span className="inline-flex items-center gap-1 text-3xs font-normal text-muted-foreground">
      <Icon aria-hidden className={cn("size-3 shrink-0", tone)} />
      {label}
    </span>
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
  const tBg = useTranslations("Folder.chat.backgroundTasks")

  const isRunning =
    part.state === "input-available" || part.state === "input-streaming"
  const isError = part.state === "output-error"

  // Shared with the aux panel's native-sub-agent rows: one parser for the
  // launch payload, so the two surfaces never drift on field spellings.
  const launchFields = useMemo(
    () => parseSubAgentLaunchFields(part.input ?? null),
    [part.input]
  )
  const {
    subagentType,
    description,
    prompt,
    model,
    agentId,
    isCursorTask,
    // The launch capsule's own status describes the spawn, so the codex
    // child's outcome (codexSubagentState) needs its own chip — see
    // `SubAgentLaunchFields`.
    isCodexSubagentLaunch: isCodexSubagent,
    codexSubagentState,
  } = launchFields

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
    () => parseTaskOutcomeEnvelope(part.output, isCursorTask),
    [part.output, isCursorTask]
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

  const transcriptTail = useMemo(
    () => (part.agentTranscript ?? []).slice(-AGENT_TRANSCRIPT_RENDER_TAIL),
    [part.agentTranscript]
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

  // Grok live sub-agent ticker — the only live signal of the child's work
  // (grok forwards no child chunks/tool calls). Shown while the child runs:
  // in-turn for a blocking spawn, alongside the "running in background" state
  // for a background one. Frozen values disappear with those states.
  const grokProgress = useMemo(
    () => parseGrokSubagentProgress(part.meta),
    [part.meta]
  )

  // The child's own session, when it has one (grok, codex). Available live —
  // from the spawn notification's meta / id — as well as in history, so a
  // running child can be watched while it works instead of only after it
  // reports back.
  const childSession = useMemo(
    () => childSessionOfLaunch(launchFields, part.meta, agentStats),
    [launchFields, part.meta, agentStats]
  )
  const viewerHost = useSessionViewerHost()
  const [sessionOpen, setSessionOpen] = useState(false)
  const grokProgressLine = useMemo(() => {
    if (!grokProgress) return null
    const pieces: string[] = []
    if (grokProgress.toolCallCount != null) {
      pieces.push(
        t("agentProgressTools", { count: grokProgress.toolCallCount })
      )
    }
    if (grokProgress.turnCount != null) {
      pieces.push(t("agentProgressTurns", { count: grokProgress.turnCount }))
    }
    if (grokProgress.durationMs != null) {
      pieces.push(formatDuration(grokProgress.durationMs))
    }
    if (grokProgress.contextUsagePct != null) {
      pieces.push(
        t("agentProgressContext", {
          pct: Math.round(grokProgress.contextUsagePct),
        })
      )
    }
    return pieces.length > 0 ? pieces.join(" · ") : null
  }, [grokProgress, t])

  const durationSuffix = useMemo(() => {
    if (agentStats?.total_duration_ms) {
      return formatDuration(agentStats.total_duration_ms)
    }
    if (taskOutcome?.durationMs != null) {
      return formatDuration(taskOutcome.durationMs)
    }
    return null
  }, [agentStats, taskOutcome])

  // Opening the child's transcript is capsule-level, not body-level: while it
  // runs (and, for codex, even after) the child's session is the only record of
  // its work, so the entry point must not depend on the capsule having a body
  // to expand. Same handler as the body version it replaces.
  const openChildSession = useMemo(() => {
    if (!childSession) return null
    return () =>
      // Preferred: the transcript-level host, which survives this card
      // scrolling out of the virtual list. Falls back to owning the drawer here
      // when there is no host (this part also renders inside the grok child
      // transcript, which is not virtualized).
      viewerHost
        ? viewerHost.open({
            kind: "agentSession",
            sessionId: childSession.sessionId,
            agentType: childSession.agentType,
            subagentType,
            description,
            // Keep re-reading the child's transcript from disk while its launch
            // call is unsettled or the background child is still out. A snapshot
            // once handed over — see the note on `AgentSessionRequest.live`.
            live: isRunning || isLiveBackgroundLaunch,
          })
        : setSessionOpen(true)
  }, [
    childSession,
    viewerHost,
    subagentType,
    description,
    isRunning,
    isLiveBackgroundLaunch,
  ])

  return (
    <>
      <AgentCapsule
        title={title}
        isRunning={isRunning || isLiveBackgroundLaunch || outcomeBackground}
        isError={isError || backgroundFailed || outcomeError != null}
        rightSuffix={durationSuffix}
        idBadge={agentId ? shortAgentId(agentId) : null}
        // codex only: the launch capsule's own state describes the spawn, so the
        // child's outcome needs a chip of its own. Held back while the spawn is
        // still in flight — there is nothing to report yet.
        stateBadge={
          isCodexSubagent && !isRunning ? (
            <CodexSubagentStateBadge state={codexSubagentState} />
          ) : null
        }
        headerAction={
          openChildSession ? (
            <SubagentSessionButton onClick={openChildSession} />
          ) : null
        }
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

        {/* Live subagent transcript (claude-agent-acp ≥0.63) — streaming
          text/thinking attributed to this Agent call. LIVE-only by
          construction: the store stops attaching it at settle and promotion
          never carries it, so this section disappears when the real result
          takes over below. Render is tail-bounded; the data is not. */}
        {isRunning && transcriptTail.length > 0 && (
          <div className="space-y-1.5">
            <div className="text-3xs font-medium uppercase tracking-wide text-muted-foreground/70">
              {t("agentLiveTranscript")}
            </div>
            {transcriptTail.map((entry, i) =>
              entry.type === "thinking" ? (
                entry.text.trim() ? (
                  <div
                    key={i}
                    className="whitespace-pre-wrap text-xs italic text-muted-foreground/80"
                  >
                    {entry.text}
                  </div>
                ) : null
              ) : (
                <div
                  key={i}
                  className="text-sm prose prose-sm dark:prose-invert max-w-none [&_ul]:list-inside [&_ol]:list-inside"
                >
                  <MessageResponse>{entry.text}</MessageResponse>
                </div>
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
            <Shimmer
              className="text-sm"
              duration={1}
              shineColor="var(--primary)"
            >
              {isLiveBackgroundLaunch || outcomeBackground
                ? tBg("cardRunning")
                : t("agentRunning")}
            </Shimmer>
          </div>
        )}

        {/* codex native sub-agent whose outcome codex has NOT reported. The chip
          can only say "unknown"; the caveat behind it needs a sentence, because
          the card would otherwise read as "the sub-agent finished" when all
          that finished was the launch. Settled outcomes say it in the chip
          instead and add nothing here — which is what lets a live capsule with
          no result collapse to a bare pill rather than an empty frame. */}
        {isCodexSubagent && !isRunning && codexSubagentState === null && (
          <div className="text-xs text-muted-foreground">
            {t("agentCodexLaunchOnly")}
          </div>
        )}

        {/* Grok live sub-agent ticker (`subagent_progress`) — only while the
          child is still running; the settled card renders stats/result. */}
        {(isRunning || isLiveBackgroundLaunch) && grokProgressLine && (
          <div className="text-xs text-muted-foreground">
            {grokProgressLine}
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

      {/* The drawer the header action opens when this card has to own it (no
          `SessionViewerHost` above — see `openChildSession`). A SIBLING of the
          capsule, never one of its children: the capsule counts its children to
          decide whether it has a body, so mounting the drawer inside would grow
          a chevron and a bordered frame the moment the drawer opened — and a
          bodyless capsule never renders its children at all, which is exactly
          the live case this drawer exists for. */}
      {childSession && viewerHost == null && sessionOpen && (
        <SubagentSessionDialog
          open={sessionOpen}
          onOpenChange={setSessionOpen}
          sessionId={childSession.sessionId}
          agentType={childSession.agentType}
          subagentType={subagentType}
          description={description}
          live={isRunning || isLiveBackgroundLaunch}
        />
      )}
    </>
  )
})
