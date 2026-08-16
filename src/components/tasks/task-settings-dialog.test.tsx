import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import type { WorkTaskFolderSettings } from "@/lib/types"

const getMock = vi.fn()
const getOwnMock = vi.fn()
const setMock = vi.fn().mockResolvedValue(undefined)
const deleteMock = vi.fn().mockResolvedValue(undefined)

vi.mock("@/lib/api", () => ({
  workTaskSettingsGet: (...args: unknown[]) => getMock(...args),
  workTaskSettingsGetOwn: (...args: unknown[]) => getOwnMock(...args),
  workTaskSettingsSet: (...args: unknown[]) => setMock(...args),
  workTaskSettingsDelete: (...args: unknown[]) => deleteMock(...args),
  listFolderCommands: () => Promise.resolve([]),
}))

// The agent surface probes a live CLI; none of it is under test here.
vi.mock("@/components/chat/agent-selector", () => ({
  AgentSelector: () => <div data-testid="agent-selector" />,
}))
vi.mock("@/components/automations/agent-config-section", () => ({
  AgentConfigSection: () => <div data-testid="agent-config" />,
  effectiveSelections: (
    _snapshot: unknown,
    modeId: string | null,
    configValues: Record<string, string>
  ) => ({ mode_id: modeId, config_values: configValues }),
  snapshotLabels: () => ({}),
}))
vi.mock("@/components/automations/use-agent-options", () => ({
  useAgentOptions: () => ({
    snapshot: null,
    loading: false,
    error: null,
    reload: vi.fn(),
    ensure: () => Promise.resolve(null),
  }),
}))
vi.mock("@/lib/custom-agents", () => ({
  getAgentLabel: (agent: string) => agent,
}))
vi.mock("@/stores/app-workspace-store", () => {
  const state = {
    folders: [
      {
        id: 1,
        name: "proj",
        alias: null,
        parent_id: null,
        kind: "regular",
        path: "/tmp/proj",
        default_agent_type: "claude_code",
      },
    ],
  }
  const useStore = (selector: (s: typeof state) => unknown) => selector(state)
  useStore.getState = () => state
  return { useAppWorkspaceStore: useStore }
})

import { TaskSettingsDialog } from "./task-settings-dialog"

function settings(
  overrides?: Partial<WorkTaskFolderSettings>
): WorkTaskFolderSettings {
  return {
    default_agent_type: null,
    mode_id: null,
    config_values: {},
    auto_process: false,
    max_concurrent: 2,
    merge_strategy: "squash",
    auto_merge: false,
    delete_worktree_default: true,
    ...overrides,
  }
}

function renderDialog(folderId: number | null) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <TaskSettingsDialog open onOpenChange={() => {}} folderId={folderId} />
    </NextIntlClientProvider>
  )
}

/** Wait for the initial load — Save is disabled until the settings land. */
async function saveButton() {
  const save = await screen.findByRole("button", { name: "Save" })
  await waitFor(() => expect(save).toBeEnabled())
  return save
}

async function openPromptsTab(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("tab", { name: "Prompts" }))
}

beforeEach(() => {
  getMock.mockReset().mockResolvedValue(settings())
  getOwnMock.mockReset().mockResolvedValue(settings())
  setMock.mockClear()
  deleteMock.mockClear()
})

describe("TaskSettingsDialog stage prompts", () => {
  it("saves the text typed for one stage under that stage's key", async () => {
    const user = userEvent.setup()
    renderDialog(1)
    const save = await saveButton()

    await openPromptsTab(user)
    await user.click(screen.getByRole("tab", { name: "Merge" }))
    await user.type(
      screen.getByRole("textbox", { name: "Merge" }),
      "Write the commit message in Chinese."
    )
    await user.click(save)

    await waitFor(() => expect(setMock).toHaveBeenCalled())
    const [, saved] = setMock.mock.calls[0] as [number, WorkTaskFolderSettings]
    expect(saved.stage_prompts).toEqual({
      merge: "Write the commit message in Chinese.",
    })
  })

  it("shows saved stage text and marks the stages that carry it", async () => {
    getOwnMock.mockResolvedValue(
      settings({
        stage_prompts: { all: "Reply in Chinese.", work: "Be brief." },
      })
    )
    const user = userEvent.setup()
    renderDialog(1)
    await saveButton()
    await openPromptsTab(user)

    // "All stages" is selected first, so its text is the one on screen.
    expect(screen.getByRole("textbox", { name: "All stages" })).toHaveValue(
      "Reply in Chinese."
    )
    // Configured stages carry a dot; untouched ones do not.
    expect(
      screen
        .getByRole("tab", { name: "Retry run" })
        .querySelector(".bg-primary")
    ).toBeNull()
    expect(
      screen.getByRole("tab", { name: "Task run" }).querySelector(".bg-primary")
    ).not.toBeNull()

    await user.click(screen.getByRole("tab", { name: "Task run" }))
    expect(screen.getByRole("textbox", { name: "Task run" })).toHaveValue(
      "Be brief."
    )
  })

  it("gives each stage its own example placeholder", async () => {
    const user = userEvent.setup()
    renderDialog(1)
    await saveButton()
    await openPromptsTab(user)

    const placeholders: string[] = []
    for (const stage of [
      "All stages",
      "Task run",
      "Retry run",
      "Follow-up",
      "Merge",
    ]) {
      await user.click(screen.getByRole("tab", { name: stage }))
      const box = screen.getByRole("textbox", { name: stage })
      placeholders.push(box.getAttribute("placeholder") ?? "")
    }

    expect(placeholders.every((p) => p.length > 0)).toBe(true)
    expect(new Set(placeholders).size).toBe(placeholders.length)
  })

  it("drops stages whose text is only whitespace", async () => {
    const user = userEvent.setup()
    renderDialog(1)
    const save = await saveButton()

    await openPromptsTab(user)
    await user.type(screen.getByRole("textbox", { name: "All stages" }), "   ")
    await user.click(save)

    await waitFor(() => expect(setMock).toHaveBeenCalled())
    const [, saved] = setMock.mock.calls[0] as [number, WorkTaskFolderSettings]
    expect(saved.stage_prompts).toEqual({})
  })
})

describe("TaskSettingsDialog workflow", () => {
  it("saves the auto-merge switch alongside the other landing options", async () => {
    const user = userEvent.setup()
    renderDialog(1)
    const save = await saveButton()

    await user.click(screen.getByRole("tab", { name: "Workflow" }))
    const toggle = screen.getByRole("switch", { name: "Merge automatically" })
    expect(toggle).toHaveAttribute("data-state", "unchecked")
    await user.click(toggle)
    await user.click(save)

    await waitFor(() => expect(setMock).toHaveBeenCalled())
    const [, saved] = setMock.mock.calls[0] as [number, WorkTaskFolderSettings]
    expect(saved.auto_merge).toBe(true)
    expect(saved.delete_worktree_default).toBe(true)
  })

  it("saves a chosen worktree directory, and blanks it back to the default", async () => {
    const user = userEvent.setup()
    renderDialog(1)
    const save = await saveButton()

    await user.click(screen.getByRole("tab", { name: "Workflow" }))
    const box = screen.getByRole("textbox", { name: "Worktree location" })
    expect(box).toHaveValue("")
    await user.type(box, "  ~/codeg-worktrees  ")
    await user.click(save)

    await waitFor(() => expect(setMock).toHaveBeenCalled())
    const [, saved] = setMock.mock.calls[0] as [number, WorkTaskFolderSettings]
    // Trimmed: a stray space would become part of the directory name.
    expect(saved.worktree_root).toBe("~/codeg-worktrees")

    // Clearing the box is how you go back to "next to the project folder",
    // which the engine reads off an absent value — not an empty one.
    setMock.mockClear()
    await user.clear(box)
    await user.click(save)
    await waitFor(() => expect(setMock).toHaveBeenCalled())
    const [, cleared] = setMock.mock.calls[0] as [
      number,
      WorkTaskFolderSettings,
    ]
    expect(cleared.worktree_root).toBeNull()
  })

  it("seeds the worktree directory from the stored row", async () => {
    getOwnMock.mockResolvedValue(settings({ worktree_root: "/var/trees" }))
    const user = userEvent.setup()
    renderDialog(1)
    await saveButton()

    await user.click(screen.getByRole("tab", { name: "Workflow" }))
    expect(
      screen.getByRole("textbox", { name: "Worktree location" })
    ).toHaveValue("/var/trees")
  })

  it("seeds the auto-merge switch from the stored row", async () => {
    getOwnMock.mockResolvedValue(settings({ auto_merge: true }))
    const user = userEvent.setup()
    renderDialog(1)
    await saveButton()

    await user.click(screen.getByRole("tab", { name: "Workflow" }))
    expect(
      screen.getByRole("switch", { name: "Merge automatically" })
    ).toHaveAttribute("data-state", "checked")
  })
})

describe("TaskSettingsDialog scope", () => {
  it("seeds a folder with no row of its own from the global defaults", async () => {
    getOwnMock.mockResolvedValue(null)
    getMock.mockResolvedValue(
      settings({ max_concurrent: 5, stage_prompts: { all: "From global." } })
    )
    const user = userEvent.setup()
    renderDialog(1)
    const save = await saveButton()

    // Following the global row: the form stays hidden until it is detached.
    expect(screen.queryByRole("tab", { name: "Prompts" })).toBeNull()
    await user.click(screen.getByRole("tab", { name: "Custom" }))
    await openPromptsTab(user)
    expect(screen.getByRole("textbox", { name: "All stages" })).toHaveValue(
      "From global."
    )

    await user.click(save)
    await waitFor(() => expect(setMock).toHaveBeenCalled())
    const [, saved] = setMock.mock.calls[0] as [number, WorkTaskFolderSettings]
    expect(saved.max_concurrent).toBe(5)
    expect(saved.stage_prompts).toEqual({ all: "From global." })
    expect(deleteMock).not.toHaveBeenCalled()
  })

  it("drops the folder's own row when it is set back to the global defaults", async () => {
    const user = userEvent.setup()
    renderDialog(1)
    const save = await saveButton()

    await user.click(screen.getByRole("tab", { name: "Global defaults" }))
    await user.click(save)

    await waitFor(() => expect(deleteMock).toHaveBeenCalledWith(1))
    expect(setMock).not.toHaveBeenCalled()
  })
})
