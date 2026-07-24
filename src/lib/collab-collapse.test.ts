import { describe, expect, it } from "vitest"

import { collapseLiveCollabBlocks } from "./collab-collapse"
import { parseCollabToolInput, classifyCollabOp } from "./collab-tool"
import type { LiveContentBlock } from "@/contexts/acp-connections-context"

function collabBlock(opts: {
  id: string
  /** op spelling, used as the ACP title. */
  title: string
  /** ACP tool-call status. */
  status?: string
  prompt?: string
  model?: string
  reasoningEffort?: string
  agents: Record<string, { status: string; message?: string | null }>
}): LiveContentBlock {
  const agentsStates: Record<string, unknown> = {}
  for (const [tid, s] of Object.entries(opts.agents)) {
    agentsStates[tid] = { status: s.status, message: s.message ?? null }
  }
  return {
    type: "tool_call",
    info: {
      tool_call_id: opts.id,
      title: opts.title,
      kind: "other",
      status: opts.status ?? "completed",
      content: null,
      raw_input: JSON.stringify({
        prompt: opts.prompt ?? "",
        senderThreadId: "main",
        receiverThreadIds: Object.keys(opts.agents),
        agentsStates,
        ...(opts.model ? { model: opts.model } : {}),
        ...(opts.reasoningEffort
          ? { reasoningEffort: opts.reasoningEffort }
          : {}),
        status: "inProgress",
      }),
      raw_output_chunks: [],
      raw_output_total_bytes: 0,
      locations: null,
      meta: null,
      images: [],
    },
  }
}

const text = (t: string): LiveContentBlock => ({ type: "text", text: t })

/** Find the single execution (spawn) block and read its rewritten agent state. */
function execAgent(blocks: LiveContentBlock[], agentId: string) {
  const spawn = blocks.find(
    (b) => b.type === "tool_call" && classifyCollabOp(b.info.title) === "spawn"
  )
  if (!spawn || spawn.type !== "tool_call") throw new Error("no spawn block")
  const info = parseCollabToolInput(spawn.info.raw_input)
  return {
    status: spawn.info.status,
    agent: info?.agents.find((a) => a.threadId === agentId),
  }
}

describe("collapseLiveCollabBlocks", () => {
  it("returns the same array reference when there are no collab blocks", () => {
    const input = [text("hi"), text("bye")]
    expect(collapseLiveCollabBlocks(input)).toBe(input)
  })

  it("aggregates spawn→wait→close into one execution capsule + the wait, drops close", () => {
    const input: LiveContentBlock[] = [
      collabBlock({
        id: "spawn",
        title: "spawnAgent",
        status: "completed",
        prompt: "Run the build",
        agents: { A: { status: "pendingInit" } },
      }),
      text("narration: started, waiting"),
      collabBlock({
        id: "wait",
        title: "wait",
        status: "completed",
        agents: { A: { status: "completed", message: "BUILD_RESULT" } },
      }),
      collabBlock({
        id: "close",
        title: "closeAgent",
        status: "completed",
        agents: { A: { status: "shutdown" } },
      }),
    ]
    const out = collapseLiveCollabBlocks(input)

    // close dropped; spawn + wait + narration remain (3 blocks).
    const ops = out
      .filter((b) => b.type === "tool_call")
      .map((b) =>
        b.type === "tool_call" ? classifyCollabOp(b.info.title) : ""
      )
    expect(ops).toEqual(["spawn", "wait"])
    expect(out.some((b) => b.type === "text")).toBe(true)

    // Execution capsule: pendingInit lifted to completed, NO result text on it,
    // ACP status settled to completed.
    const { status, agent } = execAgent(out, "A")
    expect(agent?.status).toBe("completed")
    expect(agent?.message).toBeNull()
    expect(status).toBe("completed")

    // The result text lives on the wait capsule.
    const wait = out.find(
      (b) => b.type === "tool_call" && classifyCollabOp(b.info.title) === "wait"
    )
    const waitInfo =
      wait?.type === "tool_call"
        ? parseCollabToolInput(wait.info.raw_input)
        : null
    expect(waitInfo?.agents[0]?.message).toBe("BUILD_RESULT")
  })

  it("marks the execution capsule in_progress while the agent is still running", () => {
    const input: LiveContentBlock[] = [
      collabBlock({
        id: "spawn",
        title: "spawnAgent",
        status: "completed",
        prompt: "go",
        agents: { A: { status: "pendingInit" } },
      }),
      collabBlock({
        id: "wait",
        title: "wait",
        status: "in_progress",
        agents: { A: { status: "running", message: "working…" } },
      }),
    ]
    const out = collapseLiveCollabBlocks(input)
    const { status, agent } = execAgent(out, "A")
    expect(agent?.status).toBe("running")
    expect(status).toBe("in_progress")
  })

  it("keeps multiple wait capsules independent and drops their closes", () => {
    const input: LiveContentBlock[] = [
      collabBlock({
        id: "spawnA",
        title: "spawnAgent",
        prompt: "task A",
        agents: { A: { status: "pendingInit" } },
      }),
      collabBlock({
        id: "spawnB",
        title: "spawnAgent",
        prompt: "task B",
        agents: { B: { status: "pendingInit" } },
      }),
      text("both started"),
      collabBlock({
        id: "wait1",
        title: "wait",
        agents: { B: { status: "completed", message: "B_RESULT" } },
      }),
      text("B back, waiting A"),
      collabBlock({
        id: "wait2",
        title: "wait",
        agents: { A: { status: "completed", message: "A_RESULT" } },
      }),
    ]
    const out = collapseLiveCollabBlocks(input)
    const waits = out.filter(
      (b) => b.type === "tool_call" && classifyCollabOp(b.info.title) === "wait"
    )
    expect(waits.length).toBe(2)
    const messages = waits.flatMap((w) =>
      w.type === "tool_call"
        ? (parseCollabToolInput(w.info.raw_input)?.agents.map(
            (a) => a.message
          ) ?? [])
        : []
    )
    expect(messages.sort()).toEqual(["A_RESULT", "B_RESULT"])
    // Two execution capsules (one per spawn).
    const spawns = out.filter(
      (b) =>
        b.type === "tool_call" && classifyCollabOp(b.info.title) === "spawn"
    )
    expect(spawns.length).toBe(2)
  })

  it("errored agent makes the execution capsule failed and keeps no-wait result", () => {
    const input: LiveContentBlock[] = [
      collabBlock({
        id: "spawn",
        title: "spawnAgent",
        prompt: "risky",
        agents: { A: { status: "pendingInit" } },
      }),
      // No wait; close carries the last status + message → execution fallback.
      collabBlock({
        id: "close",
        title: "closeAgent",
        agents: { A: { status: "errored", message: "boom" } },
      }),
    ]
    const out = collapseLiveCollabBlocks(input)
    // close has a spawn to fold into → dropped.
    expect(
      out.filter(
        (b) =>
          b.type === "tool_call" && classifyCollabOp(b.info.title) === "close"
      ).length
    ).toBe(0)
    const { status, agent } = execAgent(out, "A")
    expect(status).toBe("failed")
    expect(agent?.status).toBe("errored")
    // Never waited → fallback message kept on the execution capsule.
    expect(agent?.message).toBe("boom")
  })

  it("preserves top-level model/reasoningEffort on the execution capsule (codex-acp #304)", () => {
    const input: LiveContentBlock[] = [
      collabBlock({
        id: "spawn",
        title: "spawnAgent",
        prompt: "go",
        model: "gpt-5-codex",
        reasoningEffort: "high",
        agents: { A: { status: "pendingInit" } },
      }),
      collabBlock({
        id: "wait",
        title: "wait",
        agents: { A: { status: "completed", message: "R" } },
      }),
    ]
    const out = collapseLiveCollabBlocks(input)
    const spawn = out.find(
      (b) =>
        b.type === "tool_call" && classifyCollabOp(b.info.title) === "spawn"
    )
    // The rewrite rebuilds agentsStates but must carry top-level fields through
    // (it spreads `...parsed`), so model/effort survive onto the execution card.
    const info =
      spawn?.type === "tool_call"
        ? parseCollabToolInput(spawn.info.raw_input)
        : null
    expect(info?.model).toBe("gpt-5-codex")
    expect(info?.reasoningEffort).toBe("high")
  })

  it("keeps an orphan close that has no spawn to fold into", () => {
    const input: LiveContentBlock[] = [
      collabBlock({
        id: "close",
        title: "closeAgent",
        agents: { Z: { status: "shutdown" } },
      }),
    ]
    const out = collapseLiveCollabBlocks(input)
    expect(
      out.filter(
        (b) =>
          b.type === "tool_call" && classifyCollabOp(b.info.title) === "close"
      ).length
    ).toBe(1)
  })
})
