import { fireEvent, render, screen, within } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  getAvailableTerminalShells: vi.fn(),
  getSystemRenderingSettings: vi.fn(),
  getSystemTerminalSettings: vi.fn(),
  probeTerminalShellPath: vi.fn(),
  updateSystemRenderingSettings: vi.fn(),
  updateSystemTerminalSettings: vi.fn(),
}))

vi.mock("@/hooks/use-platform", () => ({
  usePlatform: () => ({
    platform: "linux",
    isMac: false,
    isWindows: false,
    isLinux: true,
  }),
}))

vi.mock("@/lib/platform", () => ({
  isDesktop: () => false,
}))

vi.mock("@/lib/transport", () => ({
  getActiveRemoteConnectionId: () => null,
}))

vi.mock("@/lib/updater", () => ({
  relaunchApp: vi.fn(),
}))

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}))

vi.mock("@/components/settings/delegation-settings", () => ({
  DelegationSettingsSection: () => <div data-testid="delegation-settings" />,
}))

vi.mock("@/components/ui/select", () => ({
  Select: ({
    children,
    onValueChange,
    value,
  }: {
    children: React.ReactNode
    onValueChange?: (value: string) => void
    value?: string
  }) => (
    <select
      value={value}
      onChange={(event) => onValueChange?.(event.target.value)}
    >
      {children}
    </select>
  ),
  SelectTrigger: () => null,
  SelectValue: () => null,
  SelectContent: ({ children }: { children: React.ReactNode }) => children,
  SelectItem: ({ value }: { children: React.ReactNode; value: string }) => (
    <option value={value}>{value}</option>
  ),
}))

import enMessages from "@/i18n/messages/en.json"
import {
  getAvailableTerminalShells,
  getSystemTerminalSettings,
} from "@/lib/api"
import { GeneralSettings } from "./general-settings"

const mockGetAvailableTerminalShells = vi.mocked(getAvailableTerminalShells)
const mockGetSystemTerminalSettings = vi.mocked(getSystemTerminalSettings)

function renderWithIntl() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <GeneralSettings />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  localStorage.clear()
  mockGetAvailableTerminalShells.mockReset()
  mockGetSystemTerminalSettings.mockReset()

  mockGetSystemTerminalSettings.mockResolvedValue({
    default_shell: null,
  })
  mockGetAvailableTerminalShells.mockResolvedValue({
    options: [
      {
        id: "system",
        label_key: "terminalSystemDefault",
        value: null,
        exists: true,
        accepts_custom_path: false,
      },
    ],
    resolved_shell: "/bin/sh",
  })
})

describe("GeneralSettings", () => {
  it("renders without WorkspaceProvider and persists workspace layout changes", async () => {
    localStorage.setItem("workspace:layout-mode", "files")

    renderWithIntl()

    const heading = await screen.findByRole("heading", {
      name: "Workspace Layout",
    })
    const section = heading.closest("section")

    expect(section).not.toBeNull()

    const layoutSelect = within(section as HTMLElement).getByDisplayValue(
      "files"
    )

    fireEvent.change(layoutSelect, { target: { value: "fusion" } })

    expect(localStorage.getItem("workspace:layout-mode")).toBe("fusion")
    expect(within(section as HTMLElement).getByDisplayValue("fusion")).toBe(
      layoutSelect
    )
  })
})
