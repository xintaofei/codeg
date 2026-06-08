import type {
  ActiveDelegationState,
  AvailableCommandInfo,
  ConnectionStatus,
  LiveContentBlock as WireLiveContentBlock,
  LiveMessage as WireLiveMessage,
  LiveSessionSnapshot,
  PromptCapabilitiesInfo,
  SessionConfigOptionInfo,
  SessionModeStateInfo,
  SessionUsageUpdateInfo,
  ToolCallState,
} from "@/lib/types"

import type {
  LiveContentBlock as LocalLiveContentBlock,
  LiveMessage as LocalLiveMessage,
  PendingPermission,
  PendingUserMessage,
  ToolCallInfo,
} from "@/contexts/acp-connections-context"

/**
 * Snapshot-derived subset of ConnectionState. Fields not present here
 * (pendingQuestion, claudeApiRetry, error, contextKey, agentType,
 * workingDir) are frontend-only or set elsewhere and must not be touched
 * by HYDRATE_FROM_SNAPSHOT.
 */
export interface SnapshotPatch {
  // Carries the snapshot's source connection_id so the reducer can reject
  // applying it when the connection at the target contextKey was
  // disconnected and replaced (different connectionId) between the
  // snapshot fetch start and its async response. Without this guard the
  // eventSeq race window allows an old connection's snapshot to overwrite
  // a freshly-started replacement at the same contextKey.
  connectionId: string
  status: ConnectionStatus
  sessionId: string | null
  modes: SessionModeStateInfo | null
  configOptions: SessionConfigOptionInfo[] | null
  availableCommands: AvailableCommandInfo[] | null
  usage: SessionUsageUpdateInfo | null
  liveMessage: LocalLiveMessage | null
  pendingPermission: PendingPermission | null
  /** In-flight user prompt carried by the snapshot, so a client attaching
   *  mid-turn can synthesize the user turn (Bug-2 / cross-client viewing).
   *  `null` when no turn is in flight. */
  pendingUserMessage: PendingUserMessage | null
  promptCapabilities: PromptCapabilitiesInfo | null
  selectorsReady: boolean
  supportsFork: boolean
  eventSeq: number
  /** Live sub-agent delegations carried by the snapshot. Consumed directly at
   *  the attach call sites to re-seed `DelegationProvider` bindings (see
   *  `seedDelegationsFromSnapshot`); the reducer does not store this on
   *  ConnectionState. `[]` when the server omitted the field. */
  activeDelegations: ActiveDelegationState[]
}

const DEFAULT_PROMPT_CAPS: PromptCapabilitiesInfo = {
  image: false,
  audio: false,
  embedded_context: false,
}

export function denormalizeSnapshot(wire: LiveSessionSnapshot): SnapshotPatch {
  const toolMap = new Map<string, ToolCallState>()
  for (const tc of wire.active_tool_calls) {
    toolMap.set(tc.id, tc)
  }

  return {
    connectionId: wire.connection_id,
    status: wire.status,
    sessionId: wire.external_id,
    modes: wire.modes,
    configOptions: wire.config_options,
    availableCommands: wire.available_commands ?? null,
    usage: wire.usage,
    liveMessage: wire.live_message
      ? denormalizeLiveMessage(wire.live_message, toolMap)
      : null,
    pendingPermission: wire.pending_permission
      ? {
          request_id: wire.pending_permission.request_id,
          // Pass the raw forwarded tool_call through unchanged.
          // `parsePermissionToolCall` walks rawInput / content / locations /
          // patch / plan to render the approval dialog — synthesizing
          // `{ description }` here would force the user to approve blind
          // after a refresh.
          tool_call: wire.pending_permission.tool_call,
          options: wire.pending_permission.options,
        }
      : null,
    pendingUserMessage: wire.pending_user_message
      ? {
          messageId: wire.pending_user_message.message_id,
          blocks: wire.pending_user_message.blocks,
        }
      : null,
    promptCapabilities: wire.prompt_capabilities ?? DEFAULT_PROMPT_CAPS,
    selectorsReady: wire.selectors_ready,
    supportsFork: wire.fork_supported,
    eventSeq: wire.event_seq,
    activeDelegations: wire.active_delegations ?? [],
  }
}

function denormalizeLiveMessage(
  wire: WireLiveMessage,
  toolMap: Map<string, ToolCallState>
): LocalLiveMessage {
  const startedAtMs = Date.parse(wire.started_at)
  return {
    id: wire.id,
    role: wire.role === "tool" ? "tool" : "assistant",
    content: wire.content
      .map((block) => denormalizeBlock(block, toolMap))
      .filter((b): b is LocalLiveContentBlock => b !== null),
    startedAt: Number.isNaN(startedAtMs) ? Date.now() : startedAtMs,
  }
}

function denormalizeBlock(
  wire: WireLiveContentBlock,
  toolMap: Map<string, ToolCallState>
): LocalLiveContentBlock | null {
  switch (wire.kind) {
    case "text":
      return { type: "text", text: wire.text }
    case "thinking":
      return { type: "thinking", text: wire.text }
    case "plan":
      // Wire `plan.entries` is `unknown` (passed through opaque from agent);
      // local shape expects PlanEntryInfo[]. We cast — backend's typed plan
      // payload is structurally identical to the local PlanEntryInfo[] shape
      // in practice (both are the agent's plan output forwarded verbatim).
      return { type: "plan", entries: wire.entries as never }
    case "tool_call_ref": {
      const tc = toolMap.get(wire.tool_call_id)
      if (!tc) {
        // Snapshot referenced a tool_call that wasn't in active_tool_calls.
        // Skip the block — the next tool_call event will recreate it.
        return null
      }
      return { type: "tool_call", info: toolStateToInfo(tc) }
    }
  }
}

function toolStateToInfo(tc: ToolCallState): ToolCallInfo {
  // Backend's structured output is collapsed into a single raw chunk for
  // hydration. Chunk history isn't recoverable from the snapshot — the
  // frontend's per-chunk delta tracking will resume from subsequent events.
  const outputChunks: string[] = []
  let outputBytes = 0
  if (tc.output) {
    const serialized =
      typeof tc.output === "string" ? tc.output : JSON.stringify(tc.output)
    outputChunks.push(serialized)
    outputBytes = serialized.length
  }
  return {
    tool_call_id: tc.id,
    title: tc.label,
    kind: tc.kind,
    status: tc.status,
    content: tc.content,
    raw_input: tc.input == null ? null : JSON.stringify(tc.input),
    raw_output_chunks: outputChunks,
    raw_output_total_bytes: outputBytes,
    locations: tc.locations ?? null,
    meta: tc.meta ?? null,
    images: tc.images ?? [],
  }
}
