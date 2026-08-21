/**
 * The trigger dialog. Its three outcomes are ANSWERS, not errors — `created`
 * hands the task back, `duplicate` offers a choice, `folder_mismatch` names
 * the remote the folder actually points at — and the coordinates it sends are
 * the backend's own (`remote.provider`, never a guess from the hostname).
 *
 * The snapshot preview is read-only by design: the prompt and its
 * untrusted-data envelope are composed server-side, so an editable box here
 * would only promise an influence it does not have.
 */
import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type {
  ForgeCreateResult,
  ForgeIssueRow,
  ForgeRemote,
  WorkTask,
} from "@/lib/types"

import { ForgeStartDialog } from "./forge-start-dialog"

const workTaskCreateFromForge = vi.fn()
vi.mock("@/lib/api", () => ({
  workTaskCreateFromForge: (...args: unknown[]) =>
    workTaskCreateFromForge(...args),
}))

const setRoute = vi.fn()
vi.mock("@/contexts/workbench-route-context", () => ({
  useWorkbenchRoute: () => ({
    routeId: "forge",
    isConversations: false,
    setRoute,
    openConversations: vi.fn(),
  }),
}))

const GITHUB: ForgeRemote = {
  server_host: "github.com",
  owner_repo: "o/r",
  remote_url: "https://github.com/o/r.git",
  provider: "github",
}
const GITLAB: ForgeRemote = {
  server_host: "gitlab.com",
  owner_repo: "group/sub/app",
  remote_url: "https://gitlab.com/group/sub/app.git",
  provider: "gitlab",
}

function row(overrides: Partial<ForgeIssueRow> = {}): ForgeIssueRow {
  return {
    number: 42,
    title: "Login times out",
    body: "steps to reproduce…",
    state: "open",
    draft: false,
    labels: [{ name: "bug", color: "#d73a4a" }],
    author: "octocat",
    updated_at: null,
    html_url: "https://github.com/o/r/issues/42",
    is_pr: false,
    comments: 0,
    ...overrides,
  }
}

function task(overrides: Partial<WorkTask> = {}): WorkTask {
  return {
    id: 5,
    folder_id: 1,
    title: "Login times out",
    ...overrides,
  } as WorkTask
}

function mount(
  item: ForgeIssueRow,
  remote: ForgeRemote = GITHUB,
  handlers: { onClose?: () => void; onCreated?: (t: WorkTask) => void } = {}
) {
  const onClose = handlers.onClose ?? vi.fn()
  const onCreated = handlers.onCreated ?? vi.fn()
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <ForgeStartDialog
        row={item}
        remote={remote}
        folderId={7}
        onClose={onClose}
        onCreated={onCreated}
      />
    </NextIntlClientProvider>
  )
  return { onClose, onCreated }
}

beforeEach(() => {
  vi.clearAllMocks()
})

describe("ForgeStartDialog", () => {
  it("sends the backend's own coordinates plus the typed instruction", async () => {
    const user = userEvent.setup()
    const created: ForgeCreateResult = { outcome: "created", task: task() }
    workTaskCreateFromForge.mockResolvedValueOnce(created)
    const { onCreated } = mount(row())

    await user.type(screen.getByRole("textbox"), "  start with the tests  ")
    await user.click(screen.getByRole("button", { name: "Create task" }))

    await waitFor(() => expect(onCreated).toHaveBeenCalledWith(created.task))
    expect(workTaskCreateFromForge).toHaveBeenCalledWith({
      folder_id: 7,
      source: {
        kind: "issue",
        // Whatever this host is was decided server-side; a guess here would
        // build a source key that matches no task's provenance.
        provider: "github",
        server_host: "github.com",
        owner_repo: "o/r",
        number: 42,
      },
      snapshot: {
        title: "Login times out",
        body: "steps to reproduce…",
        // NAMES: the snapshot is text handed to an agent, so the label's
        // colour is dropped on the way in rather than travelling as an object
        // the envelope would have to stringify.
        labels: ["bug"],
        author: "octocat",
      },
      instruction: "start with the tests",
      force: false,
    })
  })

  it("carries a blank instruction as null and marks a PR row as a pr", async () => {
    const user = userEvent.setup()
    workTaskCreateFromForge.mockResolvedValueOnce({
      outcome: "created",
      task: task(),
    })
    mount(row({ is_pr: true }))

    await user.click(screen.getByRole("button", { name: "Create task" }))
    await waitFor(() => expect(workTaskCreateFromForge).toHaveBeenCalled())
    const payload = workTaskCreateFromForge.mock.calls[0][0]
    expect(payload.instruction).toBeNull()
    expect(payload.source.kind).toBe("pr")
  })

  it("turns a duplicate into a choice, and 'create anyway' forces", async () => {
    const user = userEvent.setup()
    workTaskCreateFromForge
      .mockResolvedValueOnce({
        outcome: "duplicate",
        existing: task({ id: 9, title: "Login times out", status: "running" }),
      })
      .mockResolvedValueOnce({ outcome: "created", task: task() })
    const { onCreated } = mount(row())

    await user.click(screen.getByRole("button", { name: "Create task" }))
    await screen.findByText(/An active task already handles this item/)
    // The instruction box is gone — the decision on screen is now which task
    // to keep, not what to tell the agent.
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument()

    await user.click(screen.getByRole("button", { name: "Create anyway" }))
    await waitFor(() => expect(onCreated).toHaveBeenCalledTimes(1))
    expect(workTaskCreateFromForge.mock.calls[1][0].force).toBe(true)
  })

  it("sends a duplicate's 'view existing' to the board", async () => {
    const user = userEvent.setup()
    workTaskCreateFromForge.mockResolvedValueOnce({
      outcome: "duplicate",
      existing: task({ id: 9 }),
    })
    const { onClose } = mount(row())

    await user.click(screen.getByRole("button", { name: "Create task" }))
    await user.click(
      await screen.findByRole("button", { name: "View the existing task" })
    )
    expect(onClose).toHaveBeenCalledTimes(1)
    expect(setRoute).toHaveBeenCalledWith("tasks")
  })

  it("names the remote the folder actually points at on a mismatch", async () => {
    const user = userEvent.setup()
    workTaskCreateFromForge.mockResolvedValueOnce({
      outcome: "folder_mismatch",
      folder_remote: {
        ...GITHUB,
        owner_repo: "other/repo",
      },
    })
    const { onCreated } = mount(row())

    await user.click(screen.getByRole("button", { name: "Create task" }))
    // Naming it is the whole answer: the picker in the toolbar is where the
    // user switches folders, and a mismatch with no name is unactionable.
    await screen.findByText(/github\.com\/other\/repo/)
    expect(onCreated).not.toHaveBeenCalled()
  })

  it("survives a mismatch that could not name a remote at all", async () => {
    const user = userEvent.setup()
    workTaskCreateFromForge.mockResolvedValueOnce({
      outcome: "folder_mismatch",
      folder_remote: null,
    })
    mount(row())
    await user.click(screen.getByRole("button", { name: "Create task" }))
    await screen.findByText(/not this issue's repository/)
  })

  it("reports a transport failure in place rather than closing", async () => {
    const user = userEvent.setup()
    workTaskCreateFromForge.mockRejectedValueOnce(new Error("network down"))
    const { onClose, onCreated } = mount(row())

    await user.click(screen.getByRole("button", { name: "Create task" }))
    await screen.findByText("network down")
    expect(onClose).not.toHaveBeenCalled()
    expect(onCreated).not.toHaveBeenCalled()
    // Still submittable — the failure was the network, not the request.
    expect(screen.getByRole("button", { name: "Create task" })).toBeEnabled()
  })

  it("promises a merge request on GitLab and a pull request on GitHub", () => {
    const { unmount } = render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <ForgeStartDialog
          row={row({ is_pr: true })}
          remote={GITLAB}
          folderId={7}
          onClose={vi.fn()}
          onCreated={vi.fn()}
        />
      </NextIntlClientProvider>
    )
    expect(screen.getByText(/merge request's head/)).toBeInTheDocument()
    expect(screen.getByText(/gitlab\.com\/group\/sub\/app/)).toBeInTheDocument()
    unmount()

    mount(row({ is_pr: true }), GITHUB)
    expect(screen.getByText(/pull request's head/)).toBeInTheDocument()
  })

  it("previews the item body read-only, and says so when there is none", async () => {
    const user = userEvent.setup()
    mount(row({ body: null }))
    await user.click(
      screen.getByRole("button", {
        name: "Preview the issue content the task will carry",
      })
    )
    const preview = await screen.findByText("(no description)")
    // Read-only: this is the DATA the server-side envelope wraps, not a
    // second composer.
    expect(preview.tagName).not.toBe("TEXTAREA")
    expect(preview).not.toHaveAttribute("contenteditable", "true")
  })
})
