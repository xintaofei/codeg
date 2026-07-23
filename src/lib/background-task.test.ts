import { describe, expect, it } from "vitest"

import {
  buildBackgroundTaskRows,
  isBackgroundTaskToolCall,
  parseBackgroundLaunch,
  parseBackgroundTaskEnvelope,
} from "@/lib/background-task"
import type { AdaptedToolCallPart } from "@/lib/adapters/ai-elements-adapter"

const COMPLETED = `<retrieval_status>success</retrieval_status>

<task_id>bfb5xnq1t</task_id>

<task_type>local_bash</task_type>

<status>completed</status>

<exit_code>0</exit_code>

<output>
running 0 tests
test result: ok. 0 passed
</output>`

const RUNNING = `<retrieval_status>timeout</retrieval_status>

<task_id>bfb5xnq1t</task_id>

<task_type>local_bash</task_type>

<status>running</status>`

const FAILED = `<retrieval_status>success</retrieval_status>

<task_id>bxx</task_id>

<task_type>local_bash</task_type>

<status>completed</status>

<exit_code>1</exit_code>

<output>error: build failed</output>`

const STOP_JSON = JSON.stringify({
  message: "Successfully stopped task: bebna5yf2 (cd /x; echo hi)",
  task_id: "bebna5yf2",
  task_type: "local_bash",
  command: "cd /x; echo hi",
})

const LAUNCH =
  "Command running in background with ID: be7lh91re. Output is being written to: /private/tmp/x/tasks/be7lh91re.output. You will be notified when it completes."

function poll(over: Partial<AdaptedToolCallPart> = {}): AdaptedToolCallPart {
  return {
    type: "tool-call",
    toolCallId: "c1",
    toolName: "TaskOutput",
    input: JSON.stringify({
      task_id: "bfb5xnq1t",
      block: true,
      timeout: 240000,
    }),
    output: COMPLETED,
    state: "output-available",
    ...over,
  }
}

describe("parseBackgroundTaskEnvelope", () => {
  it("parses a completed poll envelope", () => {
    const env = parseBackgroundTaskEnvelope(COMPLETED)
    expect(env).not.toBeNull()
    expect(env!.kind).toBe("poll")
    expect(env!.taskId).toBe("bfb5xnq1t")
    expect(env!.taskType).toBe("local_bash")
    expect(env!.status).toBe("completed")
    expect(env!.exitCode).toBe(0)
    expect(env!.output).toContain("test result: ok")
  })

  it("parses a still-running poll (no exit code / output)", () => {
    const env = parseBackgroundTaskEnvelope(RUNNING)
    expect(env!.kind).toBe("poll")
    expect(env!.status).toBe("running")
    expect(env!.retrievalStatus).toBe("timeout")
    expect(env!.exitCode).toBeNull()
    expect(env!.output).toBeNull()
  })

  it("parses a non-zero exit code", () => {
    expect(parseBackgroundTaskEnvelope(FAILED)!.exitCode).toBe(1)
  })

  it("parses a TaskStop ack JSON", () => {
    const env = parseBackgroundTaskEnvelope(STOP_JSON)
    expect(env!.kind).toBe("stop")
    expect(env!.taskId).toBe("bebna5yf2")
    expect(env!.command).toBe("cd /x; echo hi")
    expect(env!.status).toBe("stopped")
  })

  it("returns null for unrelated text and for the launch notice", () => {
    expect(parseBackgroundTaskEnvelope("just some text")).toBeNull()
    expect(parseBackgroundTaskEnvelope(LAUNCH)).toBeNull()
    expect(
      parseBackgroundTaskEnvelope(JSON.stringify({ tasks: [] }))
    ).toBeNull()
    expect(parseBackgroundTaskEnvelope(null)).toBeNull()
  })

  it("keeps output containing angle brackets intact (greedy close)", () => {
    const env = parseBackgroundTaskEnvelope(
      `<retrieval_status>success</retrieval_status>\n<task_id>z</task_id>\n<status>completed</status>\n<exit_code>0</exit_code>\n<output>a <b> c </output>`
    )
    expect(env!.output).toBe("a <b> c ")
  })
})

describe("parseBackgroundLaunch", () => {
  it("extracts the task id from a background launch result", () => {
    expect(parseBackgroundLaunch(LAUNCH)).toEqual({ taskId: "be7lh91re" })
  })
  it("returns null for non-launch text", () => {
    expect(parseBackgroundLaunch(COMPLETED)).toBeNull()
    expect(parseBackgroundLaunch(null)).toBeNull()
  })
})

describe("isBackgroundTaskToolCall", () => {
  it("matches by raw tool name (historical path)", () => {
    expect(
      isBackgroundTaskToolCall(poll({ toolName: "TaskOutput", output: null }))
    ).toBe(true)
    expect(
      isBackgroundTaskToolCall(
        poll({
          toolName: "TaskStop",
          input: JSON.stringify({ task_id: "x" }),
          output: null,
        })
      )
    ).toBe(true)
  })

  it("matches by output envelope (live `task` alias)", () => {
    expect(
      isBackgroundTaskToolCall(poll({ toolName: "task", input: null }))
    ).toBe(true)
  })

  it("matches an in-flight poll by input shape (no output yet)", () => {
    expect(
      isBackgroundTaskToolCall(
        poll({ toolName: "task", output: null, state: "input-available" })
      )
    ).toBe(true)
  })

  it("does NOT match a real sub-agent Agent call", () => {
    expect(
      isBackgroundTaskToolCall(
        poll({
          toolName: "Agent",
          input: JSON.stringify({
            subagent_type: "Explore",
            task_id: "x",
            block: true,
          }),
          output: "the agent's final answer",
        })
      )
    ).toBe(false)
  })

  it("does NOT match get_delegation_status or cancel_delegation", () => {
    expect(
      isBackgroundTaskToolCall(
        poll({
          toolName: "mcp__codeg-mcp__get_delegation_status",
          input: JSON.stringify({ task_ids: ["a"] }),
          output: JSON.stringify({
            tasks: [{ task_id: "a", status: "running" }],
          }),
        })
      )
    ).toBe(false)
    expect(
      isBackgroundTaskToolCall(
        poll({
          toolName: "mcp__codeg-mcp__cancel_delegation",
          input: JSON.stringify({ task_id: "a" }),
          output: "Canceled task a",
        })
      )
    ).toBe(false)
  })

  it("does NOT match TaskUpdate", () => {
    expect(
      isBackgroundTaskToolCall(
        poll({
          toolName: "TaskUpdate",
          input: JSON.stringify({ taskId: "1", status: "completed" }),
          output: "ok",
        })
      )
    ).toBe(false)
  })
})

describe("buildBackgroundTaskRows", () => {
  it("collapses repeated polls of one task into a single completed row", () => {
    const rows = buildBackgroundTaskRows([
      poll({ toolCallId: "p1", output: RUNNING, state: "output-available" }),
      poll({ toolCallId: "p2", output: COMPLETED, state: "output-available" }),
    ])
    expect(rows).toHaveLength(1)
    expect(rows[0].badge).toBe("completed")
    expect(rows[0].exitCode).toBe(0)
    expect(rows[0].pollCount).toBe(2)
    expect(rows[0].output).toContain("test result: ok")
    expect(rows[0].taskId).toBe("bfb5xnq1t")
  })

  it("keeps parallel tasks as separate rows", () => {
    const rows = buildBackgroundTaskRows([
      poll({ toolCallId: "a", output: COMPLETED }),
      poll({
        toolCallId: "b",
        output: FAILED,
        input: JSON.stringify({ task_id: "bxx", block: true }),
      }),
    ])
    expect(rows).toHaveLength(2)
    expect(rows[0].taskId).toBe("bfb5xnq1t")
    expect(rows[1].taskId).toBe("bxx")
    expect(rows[1].badge).toBe("failed")
    expect(rows[1].exitCode).toBe(1)
  })

  it("marks an in-flight poll as running", () => {
    const rows = buildBackgroundTaskRows([
      poll({ output: null, state: "input-available" }),
    ])
    expect(rows[0].badge).toBe("running")
    expect(rows[0].isInFlight).toBe(true)
  })

  it("drops an in-flight poll whose task_id has not streamed in yet", () => {
    // claude-agent-acp emits an arg-less initial tool_call (rawInput `{}`) and
    // fills the real args only on a later update, so a still-blocking live poll
    // has no envelope and no input id. It must NOT render as an anonymous row.
    const rows = buildBackgroundTaskRows([
      poll({ input: "{}", output: null, state: "input-available" }),
    ])
    expect(rows).toHaveLength(0)
  })

  it("does not stack repeated id-less in-flight re-polls into duplicate rows", () => {
    // The reported bug: N concurrent/re-issued anonymous polls each became a
    // separate "background task running" row. With one real task attributed and
    // three id-less in-flight checks, only the attributed row survives.
    const rows = buildBackgroundTaskRows([
      poll({
        toolCallId: "done",
        output: COMPLETED,
        state: "output-available",
      }),
      poll({
        toolCallId: "x1",
        input: "{}",
        output: null,
        state: "input-available",
      }),
      poll({
        toolCallId: "x2",
        input: null,
        output: null,
        state: "input-available",
      }),
      poll({
        toolCallId: "x3",
        input: "{}",
        output: null,
        state: "input-streaming",
      }),
    ])
    expect(rows).toHaveLength(1)
    expect(rows[0].taskId).toBe("bfb5xnq1t")
  })

  it("drops a promoted orphan (output-available but status still unsettled)", () => {
    // COMPLETE_TURN promotes the unpruned arg-less TaskOutput orphan: its state
    // flips to output-available, but its forwarded status stays pending. It must
    // not re-stack as an anonymous "background task running" row post-turn.
    const rows = buildBackgroundTaskRows([
      poll({
        toolCallId: "done",
        output: COMPLETED,
        state: "output-available",
      }),
      poll({
        toolCallId: "orphan",
        input: "{}",
        output: null,
        state: "output-available",
        toolStatus: "pending",
      }),
    ])
    expect(rows).toHaveLength(1)
    expect(rows[0].taskId).toBe("bfb5xnq1t")
  })

  it("still shows an id-less poll once it carries an envelope", () => {
    // A settled/output-available poll that parsed an envelope (even without a
    // resolvable id) is real content and keeps its row.
    const noIdEnvelope = `<retrieval_status>success</retrieval_status>
<status>completed</status>
<exit_code>0</exit_code>
<output>ok</output>`
    const rows = buildBackgroundTaskRows([
      poll({ input: null, output: noIdEnvelope, state: "output-available" }),
    ])
    expect(rows).toHaveLength(1)
    expect(rows[0].badge).toBe("completed")
  })

  it("derives a stopped badge + command from a TaskStop ack", () => {
    const rows = buildBackgroundTaskRows([
      poll({
        toolName: "TaskStop",
        input: JSON.stringify({ task_id: "bebna5yf2" }),
        output: STOP_JSON,
      }),
    ])
    expect(rows[0].badge).toBe("stopped")
    expect(rows[0].command).toBe("cd /x; echo hi")
  })
})
