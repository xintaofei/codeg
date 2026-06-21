import { render, screen, fireEvent } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import type { Automation } from "@/lib/types"

// ── Context + side-effect mocks ────────────────────────────────────────────
const refetch = vi.fn().mockResolvedValue(undefined)
let automations: Automation[] = []

vi.mock("@/contexts/automations-view-context", () => ({
  useAutomationsView: () => ({ automations, unseenFailures: 0, refetch }),
}))
vi.mock("@/contexts/workbench-route-context", () => ({
  useWorkbenchRoute: () => ({ openConversations: vi.fn() }),
}))
vi.mock("@/contexts/tab-context", () => ({
  useTabContext: () => ({ openTab: vi.fn() }),
}))
vi.mock("@/contexts/app-workspace-context", () => ({
  useAppWorkspace: () => ({ folders: [{ id: 1, name: "repo" }] }),
}))
vi.mock("@/lib/platform", () => ({
  subscribe: vi.fn().mockResolvedValue(() => {}),
}))
vi.mock("@/lib/api", () => ({
  automationMarkSeen: vi.fn().mockResolvedValue(undefined),
  automationCreate: vi.fn(),
  automationUpdate: vi.fn(),
  automationDelete: vi.fn(),
  automationRunNow: vi.fn(),
  automationSetEnabled: vi.fn(),
  automationCancelRun: vi.fn(),
  automationRuns: vi.fn().mockResolvedValue([]),
}))

// Stub the heavy editor (AgentSelector / config probing) — surface only the
// seeded automation name and the back affordance so we can assert page wiring.
vi.mock("./automation-editor", () => ({
  AutomationEditor: ({
    automation,
    onBackToTemplates,
  }: {
    automation: { name?: string } | null
    onBackToTemplates?: () => void
  }) => (
    <div>
      <div data-testid="editor-name">{automation?.name ?? "<blank>"}</div>
      {onBackToTemplates ? (
        <button type="button" onClick={onBackToTemplates}>
          back-link
        </button>
      ) : null}
    </div>
  ),
}))

import { AutomationsPage } from "./automations-page"

function renderPage() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <AutomationsPage />
    </NextIntlClientProvider>
  )
}

describe("AutomationsPage (empty + template flow)", () => {
  beforeEach(() => {
    automations = []
    vi.clearAllMocks()
  })

  it("shows the onboarding gallery when there are no automations", () => {
    renderPage()
    expect(
      screen.getByText(enMessages.Automations.onboardTitle)
    ).toBeInTheDocument()
    expect(screen.getByText("Blank automation")).toBeInTheDocument()
    expect(
      screen.getByText(enMessages.Automations.tplCodeReviewTitle)
    ).toBeInTheDocument()
  })

  it("seeds the editor with the template name when a template is picked", () => {
    renderPage()
    fireEvent.click(screen.getByText(enMessages.Automations.tplCodeReviewTitle))
    expect(screen.getByTestId("editor-name")).toHaveTextContent(
      enMessages.Automations.tplCodeReviewTitle
    )
    // Reached via the gallery, so the back-to-templates link is present.
    expect(screen.getByText("back-link")).toBeInTheDocument()
  })

  it("opens a blank editor when the Blank card is picked", () => {
    renderPage()
    fireEvent.click(screen.getByText("Blank automation"))
    expect(screen.getByTestId("editor-name")).toHaveTextContent("<blank>")
  })

  it("returns to the gallery from the editor via back-to-templates", () => {
    renderPage()
    fireEvent.click(screen.getByText(enMessages.Automations.tplCodeReviewTitle))
    expect(screen.getByTestId("editor-name")).toBeInTheDocument()
    fireEvent.click(screen.getByText("back-link"))
    // Gallery is shown again (onboarding hero + blank card).
    expect(screen.getByText("Blank automation")).toBeInTheDocument()
    expect(screen.queryByTestId("editor-name")).not.toBeInTheDocument()
  })
})

const FIXTURE: Automation = {
  id: 7,
  name: "Nightly sweep",
  enabled: true,
  trigger_kind: "manual",
  cron: null,
  timezone: "UTC",
  next_run_at: null,
  agent_type: "claude_code",
  root_folder_id: 1,
  isolation: "worktree_per_run",
  branch: null,
  is_remote_branch: false,
  config: {
    prompt_blocks: [{ type: "text", text: "do the thing" }],
    display_text: "do the thing",
    config_values: {},
  },
  last_run_at: null,
  last_run_status: null,
  last_run_conversation_id: null,
  unseen_failures: 0,
  created_at: "2026-06-01T00:00:00Z",
  updated_at: "2026-06-01T00:00:00Z",
}

describe("AutomationsPage (master-detail)", () => {
  beforeEach(() => {
    automations = [FIXTURE]
    vi.clearAllMocks()
  })

  it("can open the gallery from New and cancel back to the detail", () => {
    renderPage()
    // Detail of the only automation is shown (name appears in list + detail).
    expect(screen.getAllByText("Nightly sweep").length).toBeGreaterThan(0)
    // Enter the gallery, then cancel back.
    fireEvent.click(screen.getByText(enMessages.Automations.new))
    expect(
      screen.getByText(enMessages.Automations.startFromTemplate)
    ).toBeInTheDocument()
    fireEvent.click(screen.getByText(enMessages.Automations.cancel))
    // Back to detail; the gallery heading is gone.
    expect(screen.getAllByText("Nightly sweep").length).toBeGreaterThan(0)
    expect(
      screen.queryByText(enMessages.Automations.startFromTemplate)
    ).not.toBeInTheDocument()
  })
})
