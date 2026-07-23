import { render, screen, cleanup } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { afterEach, describe, expect, it, vi } from "vitest"

import { AgentDiagnosticsDialog } from "./agent-diagnostics-dialog"
import { acpEnvDiagnostics } from "@/lib/api"
import { copyTextToClipboard } from "@/lib/utils"
import type { AgentDiagnosticsReport, AgentType } from "@/lib/types"

// next-intl: return the key so we can assert on verdict.<code> / button keys.
vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

const toastSuccess = vi.fn()
vi.mock("sonner", () => ({
  toast: { success: (m: string) => toastSuccess(m), error: vi.fn() },
}))

vi.mock("@/lib/api", () => ({
  acpEnvDiagnostics: vi.fn(),
}))

// Keep cn() real; only stub the clipboard write.
vi.mock("@/lib/utils", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/utils")>()),
  copyTextToClipboard: vi.fn().mockResolvedValue(true),
}))

const REPORT: AgentDiagnosticsReport = {
  generated_at: "2026-07-22 10:00:00 +0800",
  agent_type: "codex" as AgentType,
  verdict: {
    level: "fail",
    code: "user_prefix_not_on_path",
    summary: "Installed into the fallback prefix.",
  },
  sections: [
    {
      title: "Node / npm",
      checks: [
        {
          label: "node",
          value: "v20.11.1 (/usr/bin/node)",
          status: "ok",
          hint: null,
        },
        {
          label: "codex-acp (resolve_npx_command)",
          value: "NOT RESOLVED",
          status: "fail",
          hint: "the new-session page checks this",
        },
      ],
    },
  ],
  plain_text:
    "===== Codeg environment diagnostics =====\nverdict [user_prefix_not_on_path]",
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe("AgentDiagnosticsDialog", () => {
  it("runs on open and renders the verdict and check rows", async () => {
    vi.mocked(acpEnvDiagnostics).mockResolvedValue(REPORT)

    render(
      <AgentDiagnosticsDialog
        open
        onOpenChange={() => {}}
        agentType={"codex" as AgentType}
      />
    )

    // Localized verdict via code (mock echoes the key).
    expect(
      await screen.findByText("verdict.user_prefix_not_on_path")
    ).toBeTruthy()
    // Structured check rows render label + value.
    expect(screen.getByText("codex-acp (resolve_npx_command)")).toBeTruthy()
    expect(screen.getByText("NOT RESOLVED")).toBeTruthy()
    // Probe was invoked with the target agent.
    expect(vi.mocked(acpEnvDiagnostics)).toHaveBeenCalledWith("codex")
  })

  it("copies the backend plain_text blob and toasts", async () => {
    vi.mocked(acpEnvDiagnostics).mockResolvedValue(REPORT)
    const user = userEvent.setup()

    render(<AgentDiagnosticsDialog open onOpenChange={() => {}} />)

    await screen.findByText("verdict.user_prefix_not_on_path")
    await user.click(screen.getByText("copyAll"))

    expect(vi.mocked(copyTextToClipboard)).toHaveBeenCalledWith(
      REPORT.plain_text
    )
    expect(toastSuccess).toHaveBeenCalledWith("copied")
  })
})
