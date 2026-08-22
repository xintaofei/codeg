import { render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"
import type { PkContestant } from "@/stores/pk-arena-store"
import { PkJudgePanel } from "./pk-judge-panel"

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

vi.mock("@/components/agent-icon", () => ({
  AgentIcon: ({ agentType }: { agentType: string }) => (
    <span data-agent={agentType} />
  ),
}))

describe("PkJudgePanel", () => {
  it("renders repeated agent types as distinct contestant slots", () => {
    const consoleError = vi
      .spyOn(console, "error")
      .mockImplementation(() => undefined)

    render(
      <PkJudgePanel
        judgeStatus="done"
        judgeAgent="codex"
        contestants={
          [
            {
              slot: 2,
              agentType: "qoder",
              label: "Model A",
              status: "done",
            },
            {
              slot: 4,
              agentType: "qoder",
              label: "Model B",
              status: "done",
            },
          ] as PkContestant[]
        }
        judgeResult={{
          scores: [
            {
              agentType: "qoder",
              score: 46,
              rank: 3,
              comment: "first model",
            },
            {
              agentType: "qoder",
              score: 42,
              rank: 4,
              comment: "second model",
            },
          ],
          summary: "done",
          rawText: "",
        }}
      />
    )

    expect(consoleError.mock.calls.flat().join(" ")).not.toContain("same key")
    expect(screen.getByText("Model A")).toBeInTheDocument()
    expect(screen.getByText("Model B")).toBeInTheDocument()
    consoleError.mockRestore()
  })
})
