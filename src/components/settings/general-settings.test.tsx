import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getSystemTerminalSettings: vi.fn(async () => ({ default_shell: null })),
  getAvailableTerminalShells: vi.fn(async () => ({
    resolved_shell: "/bin/zsh",
    options: [
      {
        id: "system",
        label_key: "terminalSystemDefault",
        value: null,
        exists: true,
        accepts_custom_path: false,
      },
      {
        id: "custom",
        label_key: "terminalShellCustom",
        value: null,
        exists: true,
        accepts_custom_path: true,
      },
    ],
  })),
  getSystemRenderingSettings: vi.fn(async () => ({
    disable_hardware_acceleration: false,
  })),
  updateSystemRenderingSettings: vi.fn(async (v: unknown) => v),
  updateSystemTerminalSettings: vi.fn(async (v: unknown) => v),
  probeTerminalShellPath: vi.fn(async () => true),
  getDelegationSettings: vi.fn(async () => ({
    enabled: false,
    depth_limit: 1,
    completed_cache_max_mb: 512,
    agent_defaults: {},
  })),
  setDelegationSettings: vi.fn(async (v: unknown) => v),
  acpListAgents: vi.fn(async () => []),
  getFeedbackSettings: vi.fn(async () => ({ enabled: false })),
  setFeedbackSettings: vi.fn(async (v: unknown) => v),
  getQuestionSettings: vi.fn(async () => ({ enabled: true })),
  setQuestionSettings: vi.fn(async (v: unknown) => v),
  getSessionInfoSettings: vi.fn(async () => ({ enabled: true })),
  setSessionInfoSettings: vi.fn(async (v: unknown) => v),
  getChatAuthoringSettings: vi.fn(async () => ({
    automations_enabled: false,
    work_tasks_enabled: false,
  })),
  setChatAuthoringSettings: vi.fn(async (v: unknown) => v),
}))

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }))
vi.mock("@/lib/platform", () => ({ isDesktop: () => true }))
vi.mock("@/lib/transport", () => ({ getActiveRemoteConnectionId: () => null }))
// Windows, so the rendering section (desktop + Windows only) is on screen.
vi.mock("@/hooks/use-platform", () => ({
  usePlatform: () => ({
    platform: "windows",
    isMac: false,
    isWindows: true,
    isLinux: false,
  }),
}))
vi.mock("@/lib/updater", () => ({ relaunchApp: vi.fn() }))
vi.mock("@/hooks/use-feedback-enabled", () => ({
  primeFeedbackEnabled: vi.fn(),
}))

import { GeneralSettings } from "./general-settings"
import enMessages from "@/i18n/messages/en.json"

/**
 * The page is a stack of sections rendered through the shared
 * `SettingsSection` / `SettingCard` / `SettingRow` grammar, so what is worth
 * pinning is the wiring that grammar carries: every row's label resolves to the
 * control it names (a `SettingRow` with the `htmlFor` left off still looks
 * right and silently loses the association), and each section actually mounts.
 */
describe("GeneralSettings", () => {
  it("mounts every section and wires each row's label to its control", async () => {
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <GeneralSettings />
      </NextIntlClientProvider>
    )

    // Terminal section: the heading itself names the picker.
    const shell = await screen.findByLabelText("Default Terminal")
    expect(shell).toBeInTheDocument()
    expect(screen.getByText("Currently using: /bin/zsh")).toBeInTheDocument()

    // Rendering section: checkbox → Switch.
    const hwAccel = screen.getByLabelText("Disable hardware acceleration")
    expect(hwAccel).toHaveAttribute("role", "switch")
    expect(hwAccel).toHaveAttribute("data-state", "unchecked")
    fireEvent.click(hwAccel)
    await waitFor(() =>
      expect(hwAccel).toHaveAttribute("data-state", "checked")
    )

    // Every child section mounted. A section that is one option is titled by
    // that option, so these double as the labels asserted above.
    for (const heading of [
      "Default Terminal",
      "Disable hardware acceleration",
      "Notification sounds",
      "Multi-Agent Collaboration",
      "In-conversation tools",
    ]) {
      expect(screen.getByRole("heading", { name: heading })).toBeInTheDocument()
    }

    // Sibling toggles keep their label association through SettingRow.
    expect(screen.getByLabelText("Enable delegation")).toBeInTheDocument()
    expect(screen.getByLabelText("Live Feedback")).toBeInTheDocument()
    expect(screen.getByLabelText("Ask user question")).toBeInTheDocument()
    expect(screen.getByLabelText("Get session info")).toBeInTheDocument()
    expect(screen.getByLabelText("Create automations")).toBeInTheDocument()
    expect(screen.getByLabelText("Create to-do tasks")).toBeInTheDocument()
  })
})
