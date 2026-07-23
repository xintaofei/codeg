import { type ReactElement } from "react"
import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"

import { AgentToolCallPart } from "./agent-tool-call"
import type { AdaptedContentPart } from "@/lib/adapters/ai-elements-adapter"
import enMessages from "@/i18n/messages/en.json"

type ToolCallPart = Extract<AdaptedContentPart, { type: "tool-call" }>

function renderCard(part: ToolCallPart) {
  const ui: ReactElement = (
    <AgentToolCallPart part={part} renderToolCall={() => null} />
  )
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

function basePart(
  input: string | null,
  state: ToolCallPart["state"]
): ToolCallPart {
  return {
    type: "tool-call",
    toolCallId: "call-agent",
    toolName: "agent",
    input,
    state,
  }
}

describe("AgentToolCallPart title", () => {
  it("renders the subagent_type prefix in front of the description", () => {
    renderCard(
      basePart(
        JSON.stringify({
          subagent_type: "Explore",
          description: "map the repo",
        }),
        "input-available"
      )
    )
    expect(screen.getByText("Explore: map the repo")).toBeInTheDocument()
    expect(screen.queryByText(/Sub-agent starting/)).not.toBeInTheDocument()
  })

  it("shows the description alone when subagent_type hasn't streamed in yet", () => {
    // Partial / out-of-order streamed input: the description is present but the
    // sub-agent type isn't. The placeholder must NOT be prepended to it.
    renderCard(
      basePart(
        '{"description":"map the repo"', // truncated, no subagent_type yet
        "input-streaming"
      )
    )
    expect(screen.getByText("map the repo")).toBeInTheDocument()
    expect(screen.queryByText(/Sub-agent starting/)).not.toBeInTheDocument()
  })

  it("falls back to the placeholder only when nothing has arrived", () => {
    renderCard(basePart(null, "input-available"))
    expect(screen.getByText("Sub-agent starting…")).toBeInTheDocument()
  })

  it("reads Codex's agent_type field as the prefix", () => {
    // Codex's live spawn_agent payload labels the agent with `agent_type`.
    renderCard(
      basePart(
        JSON.stringify({ agent_type: "codex", description: "do the thing" }),
        "input-available"
      )
    )
    expect(screen.getByText("codex: do the thing")).toBeInTheDocument()
  })

  it("ignores non-string subagent_type / description (no React-child crash)", () => {
    // Some hosts (e.g. CodeBuddy) can hand us a tool input where these fields
    // are objects, not strings. Rendering them directly would throw "Objects
    // are not valid as a React child"; they must be treated as absent.
    expect(() =>
      renderCard(
        basePart(
          JSON.stringify({ subagent_type: {}, description: {} }),
          "input-available"
        )
      )
    ).not.toThrow()
    expect(screen.getByText("Sub-agent starting…")).toBeInTheDocument()
  })

  it("keeps a string description when subagent_type is a non-string object", () => {
    renderCard(
      basePart(
        JSON.stringify({
          subagent_type: { nested: true },
          description: "build",
        }),
        "input-available"
      )
    )
    expect(screen.getByText("build")).toBeInTheDocument()
  })

  it("badges the codex agent_id (shortened to first UUID segment) when present", () => {
    renderCard(
      basePart(
        JSON.stringify({
          subagent_type: "worker",
          description: "build",
          agent_id: "abcd1234-uuid-9",
        }),
        "output-available"
      )
    )
    expect(screen.getByText("abcd1234")).toBeInTheDocument()
    expect(screen.queryByText("abcd1234-uuid-9")).not.toBeInTheDocument()
  })

  it("shows no agent_id badge for non-codex agents (e.g. Claude Task)", () => {
    renderCard(
      basePart(
        JSON.stringify({ subagent_type: "Explore", description: "map" }),
        "output-available"
      )
    )
    expect(screen.queryByText("abcd1234")).not.toBeInTheDocument()
  })
})

describe("AgentToolCallPart cursor task outcome envelope", () => {
  it("folds the success envelope into a duration suffix instead of a JSON body", () => {
    renderCard({
      ...basePart(
        JSON.stringify({ _toolName: "task", description: "run the build" }),
        "output-available"
      ),
      output: '{"durationMs":39894,"isBackground":false}',
    })
    expect(screen.getByText("39.9s")).toBeInTheDocument()
    // Even with the capsule body expanded, the raw envelope never renders.
    fireEvent.click(screen.getByRole("button", { name: "Completed" }))
    expect(screen.queryByText(/durationMs/)).not.toBeInTheDocument()
    expect(screen.queryByText(/isBackground/)).not.toBeInTheDocument()
  })

  it("renders the error envelope as an error box (wire status stays completed)", () => {
    renderCard({
      ...basePart(JSON.stringify({ _toolName: "task" }), "output-available"),
      output: '{"error":"Invalid arguments:\\nsubagent_type mismatch"}',
    })
    expect(screen.getByText(/Invalid arguments:/)).toBeInTheDocument()
    expect(screen.queryByText(/{"error"/)).not.toBeInTheDocument()
    // The capsule reports Error, not Completed.
    expect(screen.getByLabelText("Error")).toBeInTheDocument()
  })

  it("shows a background launch as still running instead of Completed", () => {
    renderCard({
      ...basePart(JSON.stringify({ _toolName: "task" }), "output-available"),
      output: '{"isBackground":true}',
    })
    // The completion envelope only acknowledges the launch: the pill carries
    // the running label…
    const trigger = screen.getByLabelText("Running in background")
    // …and the body shows the visible running indicator, not raw JSON.
    fireEvent.click(trigger)
    expect(screen.getByText("Running in background")).toBeInTheDocument()
    expect(screen.queryByText(/isBackground/)).not.toBeInTheDocument()
  })

  it("never folds outputs of non-cursor sub-agents (no _toolName stamp)", () => {
    // Another agent's sub-agent legitimately returning JSON error text: the
    // envelope must NOT repaint the card as failed — the text renders as-is.
    renderCard({
      ...basePart(
        JSON.stringify({ subagent_type: "Explore", description: "map" }),
        "output-available"
      ),
      output: '{"error":"not an envelope"}',
    })
    expect(screen.queryByLabelText("Error")).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Completed" }))
    expect(screen.getByText(/not an envelope/)).toBeInTheDocument()
  })

  it("keeps rendering genuine report text as the body", () => {
    renderCard({
      ...basePart(
        JSON.stringify({ subagent_type: "Explore", description: "map" }),
        "output-available"
      ),
      output: "All 3 checks passed.",
    })
    // Completed non-error capsules mount collapsed; expand to see the body.
    fireEvent.click(screen.getByRole("button", { name: "Completed" }))
    expect(screen.getByText("All 3 checks passed.")).toBeInTheDocument()
  })

  it("reads cursor's subagentType oneof case as the title prefix", () => {
    renderCard(
      basePart(
        JSON.stringify({
          _toolName: "task",
          description: "run the build",
          subagentType: { case: "generalPurpose", value: {} },
        }),
        "input-available"
      )
    )
    expect(
      screen.getByText("generalPurpose: run the build")
    ).toBeInTheDocument()
  })
})
