import { render, screen } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { useEffect } from "react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

const replace = vi.fn()
// Mutable between tests to simulate different `?tab=` / preserved params.
let currentSearch = ""

// Spy the refresh handler the active body registers with the hub's fixed
// toolbar. Hoisted so the (hoisted) vi.mock factory can capture it.
const { registeredRefresh, loadFolderHistoryMock } = vi.hoisted(() => ({
  registeredRefresh: vi.fn(),
  loadFolderHistoryMock: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  expertsOpenCentralDir: vi.fn(),
  loadFolderHistory: loadFolderHistoryMock,
  openFolder: vi.fn(),
}))

vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace, push: vi.fn() }),
  usePathname: () => "/settings/skill-packs",
  useSearchParams: () => new URLSearchParams(currentSearch),
}))

// Stub the three heavy bodies so this exercises only the hub's own concern
// (shared header + fixed toolbar + tabs + URL mirroring), not their async
// data-loading. The Experts stub registers a refresh spy so we can assert the
// toolbar's Refresh button drives whichever pack is active.
vi.mock("@/components/settings/experts-settings", () => ({
  ExpertsBody: ({
    onRegisterRefresh,
    scope,
    workspacePath,
    scopeControl,
  }: {
    onRegisterRefresh?: (fn: () => void) => void
    scope: string
    workspacePath: string | null
    scopeControl: React.ReactNode
  }) => {
    useEffect(() => {
      onRegisterRefresh?.(registeredRefresh)
    }, [onRegisterRefresh])
    return (
      <div
        data-testid="experts-body"
        data-scope={scope}
        data-workspace-path={workspacePath ?? ""}
      >
        {scopeControl}
        <button>Batch</button>
      </div>
    )
  },
}))
vi.mock("@/components/settings/science-settings", () => ({
  ScienceBody: () => <div data-testid="science-body" />,
}))
vi.mock("@/components/settings/office-tools-settings", () => ({
  OfficeToolsBody: () => <div data-testid="office-body" />,
}))
vi.mock("@/components/settings/custom-skills-settings", () => ({
  CustomSkillsBody: () => <div data-testid="custom-body" />,
}))

import { SkillPacksSettings } from "./skill-packs-settings"
import enMessages from "@/i18n/messages/en.json"

function renderHub() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <SkillPacksSettings />
    </NextIntlClientProvider>
  )
}

describe("SkillPacksSettings", () => {
  beforeEach(() => {
    replace.mockClear()
    registeredRefresh.mockClear()
    loadFolderHistoryMock.mockReset()
    loadFolderHistoryMock.mockImplementation(() => new Promise(() => {}))
    currentSearch = ""
  })

  it("renders the shared header, four tabs, and the fixed toolbar", () => {
    renderHub()
    expect(screen.getByText("Skill Packs")).toBeInTheDocument()
    expect(
      screen.getByText(enMessages.SkillPacksSettings.description)
    ).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Experts" })).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Science" })).toBeInTheDocument()
    expect(
      screen.getByRole("tab", { name: "Office Tools" })
    ).toBeInTheDocument()
    expect(screen.getByRole("tab", { name: "Custom" })).toBeInTheDocument()
    // The two toolbar buttons are fixed — present regardless of the active tab.
    expect(
      screen.getByRole("button", { name: "Open central folder" })
    ).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Refresh" })).toBeInTheDocument()
  })

  it("defaults to the Experts tab", () => {
    renderHub()
    expect(screen.getByTestId("experts-body")).toBeInTheDocument()
    expect(screen.queryByTestId("science-body")).not.toBeInTheDocument()
  })

  it("switches tab and mirrors the choice to the URL, preserving other params", async () => {
    currentSearch = "remoteConnectionId=3"
    renderHub()

    await userEvent.click(screen.getByRole("tab", { name: "Science" }))

    // Local state is the source of truth → the Science body mounts.
    expect(screen.getByTestId("science-body")).toBeInTheDocument()
    // …and the choice is mirrored to the URL without dropping other params.
    expect(replace).toHaveBeenCalledTimes(1)
    const url = replace.mock.calls[0][0] as string
    expect(url).toContain("tab=science")
    expect(url).toContain("remoteConnectionId=3")
  })

  it("honors an initial ?tab= deep-link", () => {
    currentSearch = "tab=office"
    renderHub()
    expect(screen.getByTestId("office-body")).toBeInTheDocument()
    expect(screen.queryByTestId("experts-body")).not.toBeInTheDocument()
  })

  it("switches to the Custom tab and mirrors it to the URL", async () => {
    renderHub()
    await userEvent.click(screen.getByRole("tab", { name: "Custom" }))
    expect(screen.getByTestId("custom-body")).toBeInTheDocument()
    expect(screen.queryByTestId("experts-body")).not.toBeInTheDocument()
    const url = replace.mock.calls[0][0] as string
    expect(url).toContain("tab=custom")
  })

  it("drives the active tab's registered handler from the fixed Refresh button", async () => {
    renderHub()
    await userEvent.click(screen.getByRole("button", { name: "Refresh" }))
    expect(registeredRefresh).toHaveBeenCalledTimes(1)
  })

  it("offers the Skills folder list and passes the selected project scope to the active pack", async () => {
    loadFolderHistoryMock.mockResolvedValueOnce([
      {
        id: 1,
        name: "Alpha",
        path: "D:\\Work\\Alpha",
        sort_order: 0,
        created_at: "2026-08-19T00:00:00Z",
        updated_at: "2026-08-19T00:00:00Z",
      },
    ])
    renderHub()
    const body = screen.getByTestId("experts-body")
    expect(body).toHaveAttribute("data-scope", "global")

    await userEvent.click(
      await screen.findByRole("combobox", { name: "Skill scope" })
    )
    await userEvent.click(await screen.findByText("Alpha"))

    expect(body).toHaveAttribute("data-scope", "project")
    expect(body).toHaveAttribute("data-workspace-path", "D:\\Work\\Alpha")
  })
})
