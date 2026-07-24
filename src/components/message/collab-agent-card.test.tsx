import { type ReactElement } from "react"
import { render, screen, fireEvent } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it } from "vitest"

import { CollabAgentCard } from "./collab-agent-card"
import { COLLAB_OP_KEY } from "@/lib/collab-tool"
import type { ToolCallState } from "@/lib/adapters/ai-elements-adapter"
import enMessages from "@/i18n/messages/en.json"

function renderCard(props: {
  input?: string | null
  errorText?: string | null
  state?: ToolCallState
}) {
  const ui: ReactElement = <CollabAgentCard {...props} />
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

function collabInput(o: {
  prompt?: string
  op?: string
  agents?: Record<string, { status: string; message: string | null }>
  status?: string
  model?: string
  reasoningEffort?: string
}) {
  return JSON.stringify({
    prompt: o.prompt ?? "",
    senderThreadId: "t-main",
    receiverThreadIds: Object.keys(o.agents ?? {}),
    agentsStates: o.agents ?? {},
    status: o.status ?? "inProgress",
    ...(o.model ? { model: o.model } : {}),
    ...(o.reasoningEffort ? { reasoningEffort: o.reasoningEffort } : {}),
    ...(o.op ? { [COLLAB_OP_KEY]: o.op } : {}),
  })
}

describe("CollabAgentCard", () => {
  it("titles a spawn with the task, badges the agent UUID, shows the prompt on expand", () => {
    renderCard({
      input: collabInput({
        prompt: "Build the app\nUNIQUE_BODY_TOKEN here",
        op: "spawnAgent",
        agents: { "11110000-aaaa-bbbb": { status: "running", message: null } },
      }),
      state: "input-available",
    })
    // Title = first line of the prompt.
    expect(screen.getByText("Build the app")).toBeInTheDocument()
    // The agent id is shown in the (collapsed) pill, shortened to its first
    // UUID segment.
    expect(screen.getByText("11110000")).toBeInTheDocument()
    // Body collapsed until expanded.
    expect(screen.queryByText(/UNIQUE_BODY_TOKEN/)).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText("Prompt")).toBeInTheDocument()
    expect(screen.getByText(/UNIQUE_BODY_TOKEN/)).toBeInTheDocument()
    // No per-agent status row anymore.
    expect(screen.queryByText("Running")).not.toBeInTheDocument()
  })

  it("shows the sub-agent model + reasoning effort on a spawn (codex-acp #304)", () => {
    renderCard({
      input: collabInput({
        prompt: "Build the app",
        op: "spawnAgent",
        model: "gpt-5-codex",
        reasoningEffort: "high",
        agents: { "11110000-aaaa": { status: "running", message: null } },
      }),
      state: "input-available",
    })
    // Collapsed until expanded; run-meta lives in the body.
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText("gpt-5-codex")).toBeInTheDocument()
    expect(screen.getByText("high")).toBeInTheDocument()
  })

  it("does NOT show model/effort on a wait capsule (live/reload parity)", () => {
    renderCard({
      input: collabInput({
        op: "wait",
        status: "completed",
        model: "gpt-5-codex",
        reasoningEffort: "high",
        agents: { "22220000-bbbb": { status: "completed", message: "done" } },
      }),
      state: "output-available",
    })
    fireEvent.click(screen.getByRole("button"))
    // The reconstructed wait capsule carries no model/effort, so the live wait
    // capsule must not render them either.
    expect(screen.queryByText("gpt-5-codex")).not.toBeInTheDocument()
    expect(screen.queryByText("high")).not.toBeInTheDocument()
  })

  it("gives a wait op an op-aware title and badges the returned agent UUID", () => {
    renderCard({
      input: collabInput({
        op: "wait",
        agents: { "22220000-cccc-dddd": { status: "running", message: null } },
      }),
      state: "input-available",
    })
    expect(screen.getByText("Fetching sub-agent result")).toBeInTheDocument()
    expect(screen.queryByText("Sub-agent")).not.toBeInTheDocument()
    expect(screen.getByText("22220000")).toBeInTheDocument()
  })

  it("renders a resultless wait as a bare pill, not an empty box", () => {
    // Newer codex `wait_agent` returns no per-agent message, so the capsule has
    // nothing to show. It must render as a bare, non-expandable pill — NOT an
    // empty bordered body frame (the sub-agent "white box" bug).
    renderCard({
      input: collabInput({
        op: "wait",
        status: "completed",
        agents: { "22220000-bbbb": { status: "completed", message: null } },
      }),
      state: "output-available",
    })
    expect(screen.getByText("Fetching sub-agent result")).toBeInTheDocument()
    // No collapsible trigger → bodyless capsule is a bare pill, not a button.
    expect(screen.queryByRole("button")).not.toBeInTheDocument()
  })

  it("op-aware titles survive snake_case op spellings", () => {
    renderCard({
      input: collabInput({
        op: "wait_agent",
        agents: { "t-sub": { status: "running", message: null } },
      }),
      state: "input-available",
    })
    expect(screen.getByText("Fetching sub-agent result")).toBeInTheDocument()

    renderCard({
      input: collabInput({ op: "close_agent", status: "completed" }),
      state: "output-available",
    })
    expect(screen.getByText("Closing sub-agent")).toBeInTheDocument()
  })

  it("badges multiple returned agents as first (short) + count", () => {
    renderCard({
      input: collabInput({
        op: "wait",
        status: "completed",
        agents: {
          "33330000-x": { status: "completed", message: "A done" },
          "44440000-y": { status: "completed", message: "B done" },
        },
      }),
      state: "output-available",
    })
    expect(screen.getByText("33330000 +1")).toBeInTheDocument()
  })

  it("renders a completed wait result message as bare markdown (no status row)", () => {
    renderCard({
      input: collabInput({
        op: "wait",
        status: "completed",
        agents: {
          "t-sub": {
            status: "completed",
            message: "Build succeeded with exit code `0`",
          },
        },
      }),
      state: "output-available",
    })
    // Not an error → collapsed; expand to see the result.
    fireEvent.click(screen.getByRole("button"))
    expect(
      screen.getByText(/Build succeeded with exit code/)
    ).toBeInTheDocument()
    // No status vocabulary in the body.
    expect(screen.queryByText("Completed")).not.toBeInTheDocument()
    expect(screen.queryByText("Sub-agents")).not.toBeInTheDocument()
  })

  it("treats an errored agent as an error: auto-opens and shows the message", () => {
    renderCard({
      input: collabInput({
        prompt: "risky",
        op: "spawnAgent",
        agents: { "t-sub": { status: "errored", message: "boom" } },
      }),
      state: "output-available",
    })
    // Errored → error → opens by default (no click), message visible.
    expect(screen.getByText(/boom/)).toBeInTheDocument()
    expect(screen.queryByText("Failed")).not.toBeInTheDocument()
  })

  it("treats notFound as an error and auto-opens its message", () => {
    renderCard({
      input: collabInput({
        op: "wait",
        agents: { "t-sub": { status: "notFound", message: "gone" } },
      }),
      state: "output-available",
    })
    expect(screen.getByText(/gone/)).toBeInTheDocument()
  })

  it("treats interrupted as non-error (collapsed until clicked)", () => {
    renderCard({
      input: collabInput({
        op: "wait",
        agents: { "t-sub": { status: "interrupted", message: "halted" } },
      }),
      state: "output-available",
    })
    // Not an error → collapsed by default.
    expect(screen.queryByText(/halted/)).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByText(/halted/)).toBeInTheDocument()
  })

  it("falls back to the generic label with no prompt and unknown op", () => {
    renderCard({
      input: collabInput({
        agents: { "t-sub": { status: "running", message: null } },
      }),
      state: "input-available",
    })
    expect(screen.getByText("Sub-agent")).toBeInTheDocument()
  })

  it("surfaces a failed op (no per-agent states) via error text, auto-opened", () => {
    renderCard({
      input: collabInput({ op: "wait", status: "failed" }),
      errorText: "OP_FAILED_TEXT",
      state: "output-error",
    })
    // Op-level failure with no agent rows: auto-opens and shows the error text.
    expect(screen.getByText("OP_FAILED_TEXT")).toBeInTheDocument()
    expect(screen.getByText("Fetching sub-agent result")).toBeInTheDocument()
  })

  it("auto-opens when a running op transitions to failed (same tool call)", () => {
    const wrap = (input: string, state: ToolCallState) => (
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <CollabAgentCard input={input} state={state} />
      </NextIntlClientProvider>
    )
    const { rerender } = render(
      wrap(
        collabInput({
          op: "wait",
          agents: { "t-sub": { status: "running", message: null } },
        }),
        "input-available"
      )
    )
    expect(screen.queryByText(/boom/)).not.toBeInTheDocument()

    rerender(
      wrap(
        collabInput({
          op: "wait",
          status: "failed",
          agents: { "t-sub": { status: "errored", message: "boom" } },
        }),
        "output-error"
      )
    )
    expect(screen.getByText(/boom/)).toBeInTheDocument()
  })
})
