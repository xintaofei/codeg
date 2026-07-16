import { render, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeAll, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import { CommandManageDialog } from "./command-manage-dialog"

vi.mock("@/hooks/use-mobile", () => ({ useIsMobile: () => true }))
vi.mock("@/lib/api", () => ({
  createFolderCommand: vi.fn(),
  deleteFolderCommand: vi.fn(),
  listFolderCommands: vi.fn().mockResolvedValue([]),
  reorderFolderCommands: vi.fn(),
  updateFolderCommand: vi.fn(),
}))

beforeAll(() => {
  globalThis.ResizeObserver = class ResizeObserver {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
})

describe("CommandManageDialog mobile layout", () => {
  it("stacks the command list and editor vertically", async () => {
    const { container, getByRole } = render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <CommandManageDialog
          open
          onOpenChange={vi.fn()}
          folderId={1}
          onChanged={vi.fn()}
        />
      </NextIntlClientProvider>
    )

    expect(getByRole("dialog")).toHaveClass("h-[calc(100dvh-5rem)]")
    expect(getByRole("button", { name: "New command" })).toHaveClass("w-full")
    await waitFor(() => {
      expect(
        container.ownerDocument.querySelector(
          '[data-panel-group-direction="vertical"]'
        )
      ).not.toBeNull()
    })
  })
})
