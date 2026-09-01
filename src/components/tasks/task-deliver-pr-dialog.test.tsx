import { render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import type {
  ForgeSourceMeta,
  WorkTask,
  WorkTaskFolderSettings,
} from "@/lib/types"

const deliverMock = vi.fn().mockResolvedValue("https://example.test/pull/4")
const settingsMock = vi.fn()

vi.mock("@/lib/api", () => ({
  workTaskDeliverPr: (...args: unknown[]) => deliverMock(...args),
  workTaskSettingsEffective: (...args: unknown[]) => settingsMock(...args),
}))

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn() },
}))

import { TaskDeliverPrDialog } from "./task-deliver-pr-dialog"

function meta(overrides?: Partial<ForgeSourceMeta>): ForgeSourceMeta {
  return {
    provider: "github",
    server_host: "github.com",
    api_base: "https://api.github.com",
    account_id: "acct",
    owner_repo: "acme/widget",
    number: 4,
    url: "https://github.com/acme/widget/pull/4",
    title: "Fix login",
    head_ref: "feature/login",
    ...overrides,
  }
}

function task(overrides?: Partial<WorkTask>): WorkTask {
  return {
    id: 7,
    folder_id: 1,
    title: "Fix login",
    config: null,
    status: "review",
    failure_reason: null,
    last_error: null,
    run_seq: 1,
    sort_order: 1,
    worktree_folder_id: 9,
    conversation_id: 3,
    connection_id: null,
    base_branch: "main",
    base_sha: "abc",
    work_branch: "task/7",
    cleanup_state: null,
    verdict: null,
    result_summary: null,
    files_changed: 3,
    additions: 10,
    deletions: 2,
    merge_commit: null,
    preflight: null,
    archived_at: null,
    scheduled_at: null,
    source_kind: "forge_pr",
    source_meta: meta(),
    created_at: "2026-08-01T00:00:00Z",
    updated_at: "2026-08-01T00:00:00Z",
    started_at: null,
    settled_at: null,
    finished_at: null,
    ...overrides,
  }
}

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
    auto_compact_percent: 0,
    delete_worktree_default: true,
    ...overrides,
  }
}

function renderDialog(t: WorkTask = task()) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <TaskDeliverPrDialog open onOpenChange={() => {}} task={t} />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  deliverMock.mockClear()
  settingsMock.mockReset()
})

describe("TaskDeliverPrDialog", () => {
  it("takes the worktree with the push-back when the folder default says so", async () => {
    settingsMock.mockResolvedValue(settings())
    renderDialog()

    const box = await screen.findByRole("checkbox", {
      name: /Delete the worktree after pushing/,
    })
    await waitFor(() => expect(box).toBeChecked())
    // A push-back names nothing, so the cleanup box is the only control.
    expect(screen.getAllByRole("checkbox")).toHaveLength(1)

    await userEvent.click(screen.getByRole("button", { name: "Push" }))
    await waitFor(() =>
      expect(deliverMock).toHaveBeenCalledWith(7, null, false, true)
    )
  })

  it("keeps the worktree when the box is unchecked", async () => {
    settingsMock.mockResolvedValue(settings())
    renderDialog()

    const box = await screen.findByRole("checkbox", {
      name: /Delete the worktree after pushing/,
    })
    await waitFor(() => expect(box).toBeChecked())
    await userEvent.click(box)

    await userEvent.click(screen.getByRole("button", { name: "Push" }))
    await waitFor(() =>
      expect(deliverMock).toHaveBeenCalledWith(7, null, false, false)
    )
  })

  it("starts unchecked when the folder default says keep", async () => {
    settingsMock.mockResolvedValue(settings({ delete_worktree_default: false }))
    renderDialog()

    await waitFor(() => expect(settingsMock).toHaveBeenCalledWith(1))
    expect(
      screen.getByRole("checkbox", {
        name: /Delete the worktree after pushing/,
      })
    ).not.toBeChecked()

    await userEvent.click(screen.getByRole("button", { name: "Push" }))
    await waitFor(() =>
      expect(deliverMock).toHaveBeenCalledWith(7, null, false, false)
    )
  })

  it("offers the same choice on the issue shape, alongside title and draft", async () => {
    settingsMock.mockResolvedValue(settings())
    renderDialog(
      task({
        source_kind: "forge_issue",
        source_meta: meta({ head_ref: null }),
      })
    )

    const box = await screen.findByRole("checkbox", {
      name: /Delete the worktree after delivering/,
    })
    await waitFor(() => expect(box).toBeChecked())
    expect(
      screen.getByRole("checkbox", { name: /Open as a draft/ })
    ).toBeInTheDocument()

    await userEvent.click(screen.getByRole("button", { name: "Push and open" }))
    await waitFor(() =>
      expect(deliverMock).toHaveBeenCalledWith(7, "Fix login", false, true)
    )
  })

  it("offers no worktree choice when there is no worktree left", async () => {
    settingsMock.mockResolvedValue(settings())
    renderDialog(task({ worktree_missing: true }))

    await waitFor(() => expect(settingsMock).toHaveBeenCalled())
    expect(screen.queryByRole("checkbox")).toBeNull()

    await userEvent.click(screen.getByRole("button", { name: "Push" }))
    await waitFor(() =>
      expect(deliverMock).toHaveBeenCalledWith(7, null, false, false)
    )
  })

  // The seed is one await away, and the box is the only control here that
  // destroys something — so a default arriving inside that window must not
  // land on top of an answer the user already gave.
  it("does not let the folder default overwrite a choice made while it loads", async () => {
    let resolveSettings: (s: WorkTaskFolderSettings) => void = () => {}
    settingsMock.mockReturnValue(
      new Promise<WorkTaskFolderSettings>((r) => {
        resolveSettings = r
      })
    )
    renderDialog()

    // Answered before the folder's "delete" default arrives.
    const box = screen.getByRole("checkbox", {
      name: /Delete the worktree after pushing/,
    })
    expect(box).not.toBeChecked()
    await userEvent.click(box)
    expect(box).toBeChecked()
    await userEvent.click(box)

    resolveSettings(settings({ delete_worktree_default: true }))
    await waitFor(() => expect(settingsMock).toHaveBeenCalled())
    expect(box).not.toBeChecked()

    await userEvent.click(screen.getByRole("button", { name: "Push" }))
    await waitFor(() =>
      expect(deliverMock).toHaveBeenCalledWith(7, null, false, false)
    )
  })

  it("does not carry the previous task's resolved value into the next open", async () => {
    settingsMock.mockResolvedValue(settings({ delete_worktree_default: true }))
    const { rerender } = render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TaskDeliverPrDialog open onOpenChange={() => {}} task={task()} />
      </NextIntlClientProvider>
    )
    await waitFor(() =>
      expect(
        screen.getByRole("checkbox", {
          name: /Delete the worktree after pushing/,
        })
      ).toBeChecked()
    )

    // A second task, in a folder whose default never resolves. The box must
    // read "keep" rather than inheriting the first task's answer.
    settingsMock.mockReturnValue(new Promise<WorkTaskFolderSettings>(() => {}))
    rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TaskDeliverPrDialog
          open
          onOpenChange={() => {}}
          task={task({ id: 8, folder_id: 2 })}
        />
      </NextIntlClientProvider>
    )
    await waitFor(() => expect(settingsMock).toHaveBeenCalledWith(2))
    expect(
      screen.getByRole("checkbox", {
        name: /Delete the worktree after pushing/,
      })
    ).not.toBeChecked()

    await userEvent.click(screen.getByRole("button", { name: "Push" }))
    await waitFor(() =>
      expect(deliverMock).toHaveBeenCalledWith(8, null, false, false)
    )
  })
})
