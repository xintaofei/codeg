import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  memorySettingsGet: vi.fn(),
  memorySettingsSet: vi.fn(),
  memoryKindList: vi.fn(),
  memoryKindCreate: vi.fn(),
  memoryKindUpdate: vi.fn(),
  memoryKindSetEnabled: vi.fn(),
  memoryKindDelete: vi.fn(),
  memorySearch: vi.fn(),
  memoryNodeDelete: vi.fn(),
  subscribeMemoryChanged: vi.fn(),
}))

vi.mock("sonner", () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}))

import { MemorySettings } from "./memory-settings"
import enMessages from "@/i18n/messages/en.json"
import {
  memoryKindCreate,
  memoryKindDelete,
  memoryKindList,
  memoryKindUpdate,
  memoryNodeDelete,
  memorySearch,
  memorySettingsGet,
  memorySettingsSet,
  subscribeMemoryChanged,
} from "@/lib/api"
import type {
  MemoryHit,
  MemoryKind,
  MemorySettings as MemorySettingsType,
} from "@/lib/types"

const mockMemorySettingsGet = vi.mocked(memorySettingsGet)
const mockMemorySettingsSet = vi.mocked(memorySettingsSet)
const mockMemoryKindList = vi.mocked(memoryKindList)
const mockMemoryKindCreate = vi.mocked(memoryKindCreate)
const mockMemoryKindUpdate = vi.mocked(memoryKindUpdate)
const mockMemoryKindDelete = vi.mocked(memoryKindDelete)
const mockMemorySearch = vi.mocked(memorySearch)
const mockMemoryNodeDelete = vi.mocked(memoryNodeDelete)
const mockSubscribeMemoryChanged = vi.mocked(subscribeMemoryChanged)

let memoryChangeHandler:
  | ((change: { kind: "settings" | "kinds" | "nodes" }) => void)
  | undefined

function renderWithIntl() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <MemorySettings />
    </NextIntlClientProvider>
  )
}

const defaultSettings: MemorySettingsType = {
  backend: "local_sqlite",
  scope: "project",
  external: null,
}

const defaultKinds: MemoryKind[] = [
  {
    id: 1,
    key: "decision",
    name: "Decisions",
    instruction: "Record architectural decisions",
    mode: "auto",
    builtin: true,
    enabled: true,
    created_at: "2024-01-01T00:00:00Z",
    updated_at: "2024-01-01T00:00:00Z",
  },
  {
    id: 2,
    key: "fixed_bug",
    name: "Fixed bugs",
    instruction: "Record bugs found and fixed",
    mode: "auto",
    builtin: true,
    enabled: true,
    created_at: "2024-01-01T00:00:00Z",
    updated_at: "2024-01-01T00:00:00Z",
  },
  {
    id: 10,
    key: "api_quirks",
    name: "API Quirks",
    instruction: "Record external API oddities",
    mode: "on_request",
    builtin: false,
    enabled: true,
    created_at: "2024-01-15T10:30:00Z",
    updated_at: "2024-01-15T10:30:00Z",
  },
]

beforeEach(() => {
  vi.clearAllMocks()
  mockSubscribeMemoryChanged.mockImplementation((handler) => {
    memoryChangeHandler = handler
    return Promise.resolve(() => {})
  })
  mockMemorySettingsGet.mockResolvedValue(defaultSettings)
  mockMemoryKindList.mockResolvedValue(defaultKinds)
})

describe("MemorySettings", () => {
  it("renders storage backend, scope, and memory kinds", async () => {
    renderWithIntl()

    expect(await screen.findByText("Local graph (SQLite)")).toBeInTheDocument()
    expect(screen.getByText("This project")).toBeInTheDocument()
    expect(screen.getByText("Decisions")).toBeInTheDocument()
    expect(screen.getByText("Fixed bugs")).toBeInTheDocument()
    expect(screen.getByText("API Quirks")).toBeInTheDocument()

    // Builtin badge check
    const builtinBadges = screen.getAllByText("Built-in")
    expect(builtinBadges.length).toBe(2)

    // Builtin kind delete button is disabled
    const builtinDeleteButton = screen.getByLabelText(
      "Delete Decisions (Built-in)"
    )
    expect(builtinDeleteButton).toBeDisabled()

    // Custom kind delete button is enabled
    const customDeleteButton = screen.getByLabelText("Delete API Quirks")
    expect(customDeleteButton).toBeEnabled()
  })

  it("saves storage backend and scope settings", async () => {
    mockMemorySettingsSet.mockResolvedValue({
      backend: "external_mcp",
      scope: "global",
      external: {
        server_id: "my-mcp",
        write_tool: "memory_write",
        search_tool: "memory_search",
        link_tool: "memory_link",
      },
    })

    renderWithIntl()

    await screen.findByText("Storage")

    // Switch to External MCP
    fireEvent.click(screen.getByLabelText("My MCP server"))

    // External inputs appear
    const serverInput = await screen.findByLabelText("MCP server")
    fireEvent.change(serverInput, { target: { value: "my-mcp" } })

    // Switch scope to Global
    fireEvent.click(screen.getByLabelText("All projects"))

    // Save settings
    const saveButton = screen.getByRole("button", { name: "Save settings" })
    fireEvent.click(saveButton)

    await waitFor(() => {
      expect(mockMemorySettingsSet).toHaveBeenCalledWith({
        backend: "external_mcp",
        scope: "global",
        external: {
          server_id: "my-mcp",
          write_tool: "memory_write",
          search_tool: "memory_search",
          link_tool: "memory_link",
        },
      })
    })
  })

  it("disables kinds section when backend is off", async () => {
    mockMemorySettingsGet.mockResolvedValue({
      backend: "off",
      scope: "project",
      external: null,
    })

    renderWithIntl()

    expect(
      await screen.findByText(/Memory storage is turned off/i)
    ).toBeInTheDocument()

    const addKindButton = screen.getByRole("button", {
      name: /Add your own kind/i,
    })
    expect(addKindButton).toBeDisabled()
  })

  it("updates mode for a memory kind", async () => {
    const updatedKind: MemoryKind = {
      ...defaultKinds[0],
      mode: "off",
    }
    mockMemoryKindUpdate.mockResolvedValue(updatedKind)

    renderWithIntl()

    await screen.findByText("Decisions")

    const modeTrigger = screen.getByLabelText("Decisions mode")
    fireEvent.click(modeTrigger)

    // Select "Off" mode
    const offOptions = await screen.findAllByText("Off")
    // Find the item option inside select dropdown
    const selectItem = offOptions.find(
      (el) =>
        el.getAttribute("role") === "option" || el.closest('[role="option"]')
    )
    if (selectItem) {
      fireEvent.click(selectItem)
    }

    await waitFor(() => {
      expect(mockMemoryKindUpdate).toHaveBeenCalledWith(1, {
        name: "Decisions",
        instruction: "Record architectural decisions",
        mode: "off",
      })
    })
  })

  it("creates a new custom kind via dialog", async () => {
    const newKind: MemoryKind = {
      id: 11,
      key: "user_preferences",
      name: "User Preferences",
      instruction: "Record preferences stated by user",
      mode: "on_request",
      builtin: false,
      enabled: true,
      created_at: "2024-01-20T14:30:00Z",
      updated_at: "2024-01-20T14:30:00Z",
    }
    mockMemoryKindCreate.mockResolvedValue(newKind)

    renderWithIntl()

    await screen.findByText("What to remember")

    const addKindButton = screen.getByRole("button", {
      name: /Add your own kind/i,
    })
    fireEvent.click(addKindButton)

    const nameInput = await screen.findByLabelText("Name")
    const instructionInput = screen.getByLabelText("When and what to write")
    const onRequestRadio = screen.getByLabelText(/On request/i)

    fireEvent.change(nameInput, { target: { value: "User Preferences" } })
    fireEvent.change(instructionInput, {
      target: { value: "Record preferences stated by user" },
    })
    fireEvent.click(onRequestRadio)

    const saveButton = screen.getByRole("button", { name: "Save" })
    fireEvent.click(saveButton)

    await waitFor(() => {
      expect(mockMemoryKindCreate).toHaveBeenCalledWith({
        name: "User Preferences",
        instruction: "Record preferences stated by user",
        mode: "on_request",
      })
    })

    expect(await screen.findByText("User Preferences")).toBeInTheDocument()
  })

  it("deletes a custom kind", async () => {
    mockMemoryKindDelete.mockResolvedValue(undefined)

    renderWithIntl()

    await screen.findByText("API Quirks")

    const deleteButton = screen.getByLabelText("Delete API Quirks")
    fireEvent.click(deleteButton)

    await waitFor(() => {
      expect(mockMemoryKindDelete).toHaveBeenCalledWith(10)
    })

    await waitFor(() => {
      expect(screen.queryByText("API Quirks")).not.toBeInTheDocument()
    })
  })

  it("searches memory and allows forgetting an entry", async () => {
    const searchResults: MemoryHit[] = [
      {
        score: 0.95,
        via: [],
        node: {
          id: 101,
          kind: "fixed_bug",
          title: "SQLite WAL lock contention",
          body: "SQLite WAL lock contention fixed by applying pragma busy_timeout.",
          scope: "project",
          folder_id: 1,
          provenance: {
            run_id: 4,
            step_id: "coder",
            agent_type: "codex",
            verified_by_tests: true,
            source: "auto",
          },
          created_at: "2026-09-19T10:00:00Z",
          updated_at: "2026-09-19T10:00:00Z",
          stale_at: null,
        },
      },
    ]

    mockMemorySearch.mockResolvedValue(searchResults)
    mockMemoryNodeDelete.mockResolvedValue(undefined)

    renderWithIntl()

    await screen.findByRole("heading", { name: "Search memory" })

    const searchInput = screen.getByPlaceholderText(
      "What do you want to recall?"
    )
    fireEvent.change(searchInput, { target: { value: "WAL lock" } })

    const searchButton = screen.getByRole("button", { name: "Search memory" })
    fireEvent.click(searchButton)

    await waitFor(() => {
      expect(mockMemorySearch).toHaveBeenCalledWith("WAL lock")
    })

    expect(
      await screen.findByText("SQLite WAL lock contention")
    ).toBeInTheDocument()
    expect(screen.getByText("verified by tests")).toBeInTheDocument()
    expect(
      screen.getByText(
        "SQLite WAL lock contention fixed by applying pragma busy_timeout."
      )
    ).toBeInTheDocument()

    // Forget node
    const forgetButton = screen.getByRole("button", { name: /Forget/i })
    fireEvent.click(forgetButton)

    await waitFor(() => {
      expect(mockMemoryNodeDelete).toHaveBeenCalledWith(101)
    })

    await waitFor(() => {
      expect(
        screen.queryByText("SQLite WAL lock contention")
      ).not.toBeInTheDocument()
    })
  })

  it("reloads data when memory broadcast event occurs", async () => {
    renderWithIntl()

    await screen.findByText("Decisions")

    const freshKinds: MemoryKind[] = [
      ...defaultKinds,
      {
        id: 99,
        key: "remote_kind",
        name: "Remote Kind",
        instruction: "Added remotely",
        mode: "auto",
        builtin: false,
        enabled: true,
        created_at: "2024-01-25T16:00:00Z",
        updated_at: "2024-01-25T16:00:00Z",
      },
    ]
    mockMemoryKindList.mockResolvedValue(freshKinds)

    act(() => {
      memoryChangeHandler?.({ kind: "kinds" })
    })

    expect(await screen.findByText("Remote Kind")).toBeInTheDocument()
  })
})
