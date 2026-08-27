import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type {
  AgentRulesInspectResult,
  AgentRulesRenderResult,
} from "@/lib/types"

import { AgentRulesPickerDialog } from "./agent-rules-picker-dialog"

const api = vi.hoisted(() => ({
  inspect: vi.fn(),
  render: vi.fn(),
  saveProfile: vi.fn(),
  renameProfile: vi.fn(),
  deleteProfile: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  agentRulesInspect: (...args: unknown[]) => api.inspect(...args),
  agentRulesRender: (...args: unknown[]) => api.render(...args),
  agentRulesSaveProfile: (...args: unknown[]) => api.saveProfile(...args),
  agentRulesRenameProfile: (...args: unknown[]) => api.renameProfile(...args),
  agentRulesDeleteProfile: (...args: unknown[]) => api.deleteProfile(...args),
}))

const catalog: AgentRulesInspectResult = {
  workspace: "/repo",
  nativeSources: ["AGENTS.md"],
  rules: [
    {
      id: "tests",
      name: "Testing",
      defaultOn: true,
      source: ".codeg/rules/team.md",
      line: 1,
    },
  ],
  defaultIds: ["tests"],
  sourceHash: "current-hash",
  profilePath: ".codeg/agent-rule-profiles.json",
  profilesExist: true,
  defaultProfile: "release",
  profiles: {
    release: {
      ruleIds: ["tests", "removed"],
      sourceHash: "old-hash",
      stale: true,
      missingRuleIds: ["removed"],
    },
  },
}

const rendered: AgentRulesRenderResult = {
  sourceHash: "current-hash",
  rules: catalog.rules,
  sources: [".codeg/rules/team.md"],
  text: "Run tests.\n",
  envelopeNonce: "nonce",
}

function mount(onApply = vi.fn()) {
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <AgentRulesPickerDialog
        open
        onOpenChange={() => {}}
        rootPath="/repo"
        agentType="codex"
        onApply={onApply}
      />
    </NextIntlClientProvider>
  )
  return onApply
}

beforeEach(() => {
  vi.clearAllMocks()
  api.inspect.mockResolvedValue(catalog)
  api.render.mockResolvedValue(rendered)
})

describe("AgentRulesPickerDialog", () => {
  it("blocks a profile with missing rules until the selection is revised", async () => {
    const user = userEvent.setup()
    const onApply = mount()

    expect(
      await screen.findByText(/Missing rules: removed/)
    ).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Apply once" })).toBeDisabled()

    await user.click(screen.getByRole("button", { name: "Remove missing" }))
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Apply once" })).toBeEnabled()
    )
    expect(screen.queryByText(/Missing rules: removed/)).not.toBeInTheDocument()

    await user.click(screen.getByRole("button", { name: "Apply once" }))
    await waitFor(() => expect(onApply).toHaveBeenCalledTimes(1))
    expect(api.render).toHaveBeenLastCalledWith({
      rootPath: "/repo",
      ruleIds: ["tests"],
      expectedSourceHash: "current-hash",
    })
    expect(onApply).toHaveBeenCalledWith(
      expect.objectContaining({
        ruleIds: ["tests"],
        exactText: "Run tests.\n",
      })
    )
  })
})
