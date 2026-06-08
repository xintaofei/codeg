import { StrictMode, useEffect, useState } from "react"
import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type { DirectoryEntry } from "@/lib/types"
import { DirectoryBrowserDialog } from "./directory-browser-dialog"

// Only the two filesystem reads the dialog performs are mocked; path-utils is
// left real so the production parentFsPath logic is exercised end to end.
const api = vi.hoisted(() => ({
  getHomeDirectory: vi.fn(),
  listDirectoryEntries: vi.fn(),
}))
vi.mock("@/lib/api", () => api)

const dir = (
  name: string,
  path: string,
  hasChildren = false
): DirectoryEntry => ({ name, path, hasChildren })

const onSelect = vi.fn()
const onOpenChange = vi.fn()

// Lets a test drive `open` from the outside (close / reopen) while the dialog's
// own onOpenChange (e.g. Cancel) still flips it — both go through one source of
// truth, mirroring how the real parents own the open state.
let setOpenExternal: (open: boolean) => void = () => {}

// Controlled wrapper so cancelling actually flips `open` (the parent owns it),
// which is what invalidates an in-flight confirm in the production code.
function Harness({ initialPath }: { initialPath?: string }) {
  const [open, setOpen] = useState(true)
  useEffect(() => {
    setOpenExternal = setOpen
  }, [])
  return (
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <DirectoryBrowserDialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next)
          onOpenChange(next)
        }}
        onSelect={onSelect}
        initialPath={initialPath}
      />
    </NextIntlClientProvider>
  )
}

const deferred = <T,>() => {
  let resolve: (value: T) => void = () => {}
  const promise = new Promise<T>((res) => {
    resolve = res
  })
  return { promise, resolve }
}

beforeEach(() => {
  onSelect.mockClear()
  onOpenChange.mockClear()
  setOpenExternal = () => {}
  api.getHomeDirectory.mockReset()
  api.getHomeDirectory.mockResolvedValue("/home/me")
  api.listDirectoryEntries.mockReset()
  api.listDirectoryEntries.mockResolvedValue([])
})

describe("DirectoryBrowserDialog", () => {
  it("pre-fills the initial path and confirms it without a tree click", async () => {
    api.listDirectoryEntries.mockResolvedValue([
      dir("work", "/home/me/work", true),
    ])
    render(<Harness initialPath="/home/me" />)

    await screen.findByDisplayValue("/home/me")
    const select = screen.getByRole("button", { name: "Select" })
    expect(select).toBeEnabled()

    fireEvent.click(select)

    await screen.findByDisplayValue("/home/me") // settle pending effects
    expect(onSelect).toHaveBeenCalledWith("/home/me")
    expect(onOpenChange).toHaveBeenCalledWith(false)
  })

  it("updates the path input when a directory row is clicked and confirms that path", async () => {
    api.listDirectoryEntries.mockImplementation((p: string) =>
      p === "/home/me"
        ? Promise.resolve([dir("work", "/home/me/work", true)])
        : Promise.resolve([])
    )
    render(<Harness initialPath="/home/me" />)
    // Wait for the tree row (loaded after the input is set) before clicking it.
    await screen.findByText("work")

    fireEvent.click(screen.getByText("work"))
    await screen.findByDisplayValue("/home/me/work")

    fireEvent.click(screen.getByRole("button", { name: "Select" }))
    await screen.findByDisplayValue("/home/me/work")
    expect(onSelect).toHaveBeenCalledWith("/home/me/work")
  })

  it("keeps the dialog open and shows an error when the typed path is invalid", async () => {
    api.listDirectoryEntries.mockImplementation((p: string) =>
      p === "/home/me"
        ? Promise.resolve([dir("work", "/home/me/work", true)])
        : Promise.reject(new Error("ENOENT"))
    )
    render(<Harness initialPath="/home/me" />)
    await screen.findByDisplayValue("/home/me")

    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/does/not/exist" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Select" }))

    await screen.findByText("Failed to load directory")
    expect(onSelect).not.toHaveBeenCalled()
    expect(onOpenChange).not.toHaveBeenCalledWith(false)
  })

  it("navigates up relative to the path shown in the input, not the tree root", async () => {
    api.listDirectoryEntries.mockImplementation((p: string) =>
      p === "/home/me"
        ? Promise.resolve([dir("work", "/home/me/work", true)])
        : Promise.resolve([])
    )
    render(<Harness initialPath="/home/me" />)
    await screen.findByText("work")

    // Select a child so the input is deeper than the tree root (/home/me).
    fireEvent.click(screen.getByText("work"))
    await screen.findByDisplayValue("/home/me/work")

    fireEvent.click(screen.getByTitle("Go to parent directory"))

    // Parent of the INPUT (/home/me/work) is /home/me. The old rootPath-based
    // logic would instead have jumped to /home (parent of the tree root).
    await screen.findByDisplayValue("/home/me")
    expect(screen.queryByDisplayValue("/home")).toBeNull()
  })

  it("does not commit a path when the dialog is cancelled mid-validation", async () => {
    const slow = deferred<DirectoryEntry[]>()
    api.listDirectoryEntries.mockImplementation((p: string) => {
      if (p === "/home/me")
        return Promise.resolve([dir("work", "/home/me/work", true)])
      if (p === "/slow/dir") return slow.promise
      return Promise.resolve([])
    })
    render(<Harness initialPath="/home/me" />)
    await screen.findByDisplayValue("/home/me")

    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/slow/dir" },
    })
    // Start the (still-pending) validation, then cancel before it resolves.
    fireEvent.click(screen.getByRole("button", { name: "Select" }))
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(onOpenChange).toHaveBeenCalledWith(false)

    // The validation now resolves successfully — but it is stale, so the
    // dialog must not select the cancelled path.
    await act(async () => {
      slow.resolve([])
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(onSelect).not.toHaveBeenCalled()
  })

  it("does not commit the original path when the selection changes mid-validation", async () => {
    const slow = deferred<DirectoryEntry[]>()
    api.listDirectoryEntries.mockImplementation((p: string) => {
      if (p === "/home/me")
        return Promise.resolve([dir("work", "/home/me/work", true)])
      if (p === "/slow/dir") return slow.promise
      return Promise.resolve([])
    })
    render(<Harness initialPath="/home/me" />)
    await screen.findByText("work")

    // Aim Select at an uncached path so it must validate (stays pending)...
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/slow/dir" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Select" }))

    // ...then move the selection while that validation is in flight.
    fireEvent.click(screen.getByText("work"))
    await screen.findByDisplayValue("/home/me/work")

    await act(async () => {
      slow.resolve([])
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(onSelect).not.toHaveBeenCalled()
  })

  it("does not show or confirm the previous path while a reopened dialog loads", async () => {
    const home2 = deferred<string>()
    api.getHomeDirectory.mockReset()
    api.getHomeDirectory
      .mockResolvedValueOnce("/home/me") // first open settles immediately
      .mockImplementationOnce(() => home2.promise) // reopen stays pending
    api.listDirectoryEntries.mockImplementation((p: string) =>
      p === "/home/me"
        ? Promise.resolve([dir("work", "/home/me/work", true)])
        : Promise.resolve([])
    )
    render(<Harness />) // no initialPath -> uses getHomeDirectory

    // First open settles on /home/me; pick a sub-path to create a stale value.
    await screen.findByText("work")
    fireEvent.click(screen.getByText("work"))
    await screen.findByDisplayValue("/home/me/work")

    // Close, then reopen while getHomeDirectory is still pending.
    act(() => setOpenExternal(false))
    act(() => setOpenExternal(true))

    // The stale /home/me/work must not be shown, and Select must be disabled.
    expect(screen.queryByDisplayValue("/home/me/work")).toBeNull()
    expect(screen.getByRole("button", { name: "Select" })).toBeDisabled()

    // Once the start dir resolves, the fresh path appears.
    await act(async () => {
      home2.resolve("/home/me")
      await Promise.resolve()
    })
    await screen.findByDisplayValue("/home/me")
  })

  it("ignores a stale init from a previous open", async () => {
    const home1 = deferred<string>()
    api.getHomeDirectory.mockReset()
    api.getHomeDirectory
      .mockImplementationOnce(() => home1.promise) // first open stays pending
      .mockResolvedValueOnce("/home/v2") // reopen settles immediately
    render(<Harness />)

    // Close and reopen before the first open's getHomeDirectory resolves.
    act(() => setOpenExternal(false))
    act(() => setOpenExternal(true))
    await screen.findByDisplayValue("/home/v2")

    // The stale first-open init now resolves; it must not clobber /home/v2.
    await act(async () => {
      home1.resolve("/home/v1")
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(screen.getByRole("textbox")).toHaveValue("/home/v2")
  })

  it("ignores a stale navigation (Enter) that outlives a close/reopen", async () => {
    const slow = deferred<DirectoryEntry[]>()
    api.getHomeDirectory.mockReset()
    api.getHomeDirectory
      .mockResolvedValueOnce("/home/me") // first open
      .mockResolvedValueOnce("/home/v2") // reopen
    api.listDirectoryEntries.mockImplementation((p: string) =>
      p === "/slow/dir" ? slow.promise : Promise.resolve([])
    )
    render(<Harness />)
    await screen.findByDisplayValue("/home/me")

    // Type a path and press Enter -> navigateTo() starts a slow load.
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/slow/dir" },
    })
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" })

    // Close and reopen before that navigation resolves.
    act(() => setOpenExternal(false))
    act(() => setOpenExternal(true))
    await screen.findByDisplayValue("/home/v2")

    // The stale navigation now resolves; it must not overwrite /home/v2.
    await act(async () => {
      slow.resolve([])
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(screen.getByRole("textbox")).toHaveValue("/home/v2")
  })

  it("initializes correctly under StrictMode despite effect replay", async () => {
    api.getHomeDirectory.mockReset()
    api.getHomeDirectory.mockResolvedValue("/home/me")
    api.listDirectoryEntries.mockResolvedValue([])
    render(
      <StrictMode>
        <Harness />
      </StrictMode>
    )

    // StrictMode replays mount effects; the session bump must stay idempotent
    // so the first init's async writes are not dropped as stale.
    await screen.findByDisplayValue("/home/me")
    expect(screen.getByRole("button", { name: "Select" })).toBeEnabled()
  })

  it("does not let a stale confirm clear a newer open's spinner", async () => {
    const slowA = deferred<DirectoryEntry[]>()
    const slowB = deferred<DirectoryEntry[]>()
    api.getHomeDirectory.mockReset()
    api.getHomeDirectory
      .mockResolvedValueOnce("/home/me") // open #1
      .mockResolvedValueOnce("/home/me") // reopen
    api.listDirectoryEntries.mockImplementation((p: string) => {
      if (p === "/slow/a") return slowA.promise
      if (p === "/slow/b") return slowB.promise
      return Promise.resolve([])
    })
    render(<Harness />)
    await screen.findByDisplayValue("/home/me")

    // Open #1: start a confirm that stays pending.
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/slow/a" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Select" }))

    // Close and reopen -> new session.
    act(() => setOpenExternal(false))
    act(() => setOpenExternal(true))
    await screen.findByDisplayValue("/home/me")

    // Open #2: start another confirm that stays pending -> spinner/disabled.
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/slow/b" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Select" }))
    expect(screen.getByRole("button", { name: "Select" })).toBeDisabled()

    // The stale open #1 confirm resolves; it must not clear open #2's spinner.
    await act(async () => {
      slowA.resolve([])
      await Promise.resolve()
      await Promise.resolve()
    })

    expect(screen.getByRole("button", { name: "Select" })).toBeDisabled()
    expect(onSelect).not.toHaveBeenCalled()
  })

  it("lets the newest navigation win when an older one resolves later", async () => {
    const slowOld = deferred<DirectoryEntry[]>()
    api.listDirectoryEntries.mockImplementation((p: string) =>
      p === "/nav/old" ? slowOld.promise : Promise.resolve([])
    )
    render(<Harness initialPath="/home/me" />)
    await screen.findByDisplayValue("/home/me")

    // Navigate to /nav/old (stays pending)...
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/nav/old" },
    })
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" })
    // ...then to /nav/new (resolves immediately).
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/nav/new" },
    })
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" })
    await screen.findByDisplayValue("/nav/new")

    // The older, slower navigation resolves last; it must NOT clobber /nav/new.
    await act(async () => {
      slowOld.resolve([])
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(screen.getByRole("textbox")).toHaveValue("/nav/new")
  })

  it("lets a user navigation during the initial load win over init", async () => {
    const home = deferred<string>()
    api.getHomeDirectory.mockReset()
    api.getHomeDirectory.mockImplementationOnce(() => home.promise)
    api.listDirectoryEntries.mockResolvedValue([])
    render(<Harness />) // no initialPath -> getHomeDirectory (pending)

    // While the start dir is still loading, navigate away.
    fireEvent.change(screen.getByRole("textbox"), {
      target: { value: "/typed/dir" },
    })
    fireEvent.keyDown(screen.getByRole("textbox"), { key: "Enter" })
    await screen.findByDisplayValue("/typed/dir")

    // getHomeDirectory finally resolves; init must NOT overwrite the user's path.
    await act(async () => {
      home.resolve("/home/me")
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(screen.getByRole("textbox")).toHaveValue("/typed/dir")
  })

  it("expands two folders concurrently without losing either", async () => {
    const loadA = deferred<DirectoryEntry[]>()
    const loadB = deferred<DirectoryEntry[]>()
    api.listDirectoryEntries.mockImplementation((p: string) => {
      if (p === "/home/me")
        return Promise.resolve([
          dir("a", "/home/me/a", true),
          dir("b", "/home/me/b", true),
        ])
      if (p === "/home/me/a") return loadA.promise
      if (p === "/home/me/b") return loadB.promise
      return Promise.resolve([])
    })
    render(<Harness initialPath="/home/me" />)
    await screen.findByText("a")

    // The chevron is the only nested role=button inside each row button.
    const chevron = (name: string) =>
      within(screen.getByText(name).closest("button")!).getByRole("button")
    fireEvent.click(chevron("a"))
    fireEvent.click(chevron("b"))

    await act(async () => {
      loadA.resolve([dir("a-child", "/home/me/a/a-child")])
      loadB.resolve([dir("b-child", "/home/me/b/b-child")])
      await Promise.resolve()
      await Promise.resolve()
    })

    // Both children present -> neither expand overwrote the other's snapshot.
    expect(await screen.findByText("a-child")).toBeInTheDocument()
    expect(await screen.findByText("b-child")).toBeInTheDocument()
  })
})
