import { describe, expect, it } from "vitest"

import { extractLatestPlanEntriesFromMessages } from "./agent-plan"
import type { AdaptedMessage } from "@/lib/adapters/ai-elements-adapter"

describe("extractLatestPlanEntriesFromMessages", () => {
  it("finds plan updates nested inside a goal run", () => {
    const messages: AdaptedMessage[] = [
      {
        id: "assistant-1",
        role: "assistant",
        timestamp: "2026-06-02T00:00:00.000Z",
        content: [
          {
            type: "goal-run",
            start: {
              type: "tool-call",
              toolCallId: "create-goal",
              toolName: "create_goal",
              input: JSON.stringify({ objective: "Analyze README" }),
              state: "output-available",
            },
            end: null,
            items: [
              {
                type: "tool-call",
                toolCallId: "plan",
                toolName: "update_plan",
                input: JSON.stringify({
                  plan: [{ content: "Read README", status: "completed" }],
                }),
                state: "output-available",
              },
            ],
            isRunning: true,
          },
        ],
      },
    ]

    expect(extractLatestPlanEntriesFromMessages(messages)).toEqual([
      {
        content: "Read README",
        status: "completed",
        priority: "medium",
      },
    ])
  })

  function planToolCall(
    id: string,
    entries: { content: string; status: string }[]
  ): AdaptedMessage {
    return {
      id,
      role: "assistant",
      timestamp: "2026-06-02T00:00:00.000Z",
      content: [
        {
          type: "tool-call",
          toolCallId: `${id}-plan`,
          toolName: "TodoWrite",
          input: JSON.stringify({ todos: entries }),
          state: "output-available",
        },
      ],
    } as AdaptedMessage
  }

  function userMessage(id: string): AdaptedMessage {
    return {
      id,
      role: "user",
      timestamp: "2026-06-02T00:00:00.000Z",
      content: [{ type: "text", text: "do it" }],
    } as AdaptedMessage
  }

  function assistantText(id: string): AdaptedMessage {
    return {
      id,
      role: "assistant",
      timestamp: "2026-06-02T00:00:00.000Z",
      content: [{ type: "text", text: "done" }],
    } as AdaptedMessage
  }

  it("hides a completed plan once the user has replied after it", () => {
    const messages: AdaptedMessage[] = [
      planToolCall("a1", [
        { content: "Step one", status: "completed" },
        { content: "Step two", status: "completed" },
      ]),
      userMessage("u1"),
      assistantText("a2"),
    ]
    expect(extractLatestPlanEntriesFromMessages(messages)).toEqual([])
  })

  it("keeps a completed plan that belongs to the latest agent reply", () => {
    const messages: AdaptedMessage[] = [
      userMessage("u1"),
      planToolCall("a1", [{ content: "Step one", status: "completed" }]),
    ]
    expect(extractLatestPlanEntriesFromMessages(messages)).toEqual([
      { content: "Step one", status: "completed", priority: "medium" },
    ])
  })

  it("keeps a completed plan when only assistant messages follow it", () => {
    // Consecutive assistant messages (no user message between) are the same
    // reply, so the completed plan stays visible.
    const messages: AdaptedMessage[] = [
      planToolCall("a1", [{ content: "Step one", status: "completed" }]),
      assistantText("a2"),
    ]
    expect(extractLatestPlanEntriesFromMessages(messages)).toEqual([
      { content: "Step one", status: "completed", priority: "medium" },
    ])
  })

  it("keeps an incomplete plan even after a later user message", () => {
    // Only fully completed plans are treated as stale.
    const messages: AdaptedMessage[] = [
      planToolCall("a1", [
        { content: "Step one", status: "completed" },
        { content: "Step two", status: "in_progress" },
      ]),
      userMessage("u1"),
      assistantText("a2"),
    ]
    expect(extractLatestPlanEntriesFromMessages(messages)).toEqual([
      { content: "Step one", status: "completed", priority: "medium" },
      { content: "Step two", status: "in_progress", priority: "medium" },
    ])
  })
})
