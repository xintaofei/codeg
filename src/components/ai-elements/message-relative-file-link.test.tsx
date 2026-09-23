import { fireEvent, render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// End-to-end guard for the "Windows local file link renders as [blocked]" bug
// (issue #362). Exercises the REAL Streamdown pipeline (no streamdown mock) so
// the assertions cover actual rehype `sanitize` + `harden` behavior — the layer
// that read `E:` in `E:/…` as a URL protocol, stripped the href, and let harden
// replace the link with "<name> [blocked]". Only the leaf dependencies of the
// real link-safety hook are stubbed, so the click path (badge → link-safety →
// `openFilePreview`) is genuinely exercised too.
const mocks = vi.hoisted(() => ({
  openFilePreview: vi.fn(),
  openUrl: vi.fn(),
  toastError: vi.fn(),
  isDesktop: vi.fn(() => false),
  getActiveRemoteConnectionId: vi.fn(() => null),
}))

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

vi.mock("sonner", () => ({
  toast: { error: mocks.toastError },
}))

vi.mock("@/lib/platform", () => ({
  openUrl: mocks.openUrl,
}))

vi.mock("@/lib/transport", () => ({
  isDesktop: mocks.isDesktop,
  getActiveRemoteConnectionId: mocks.getActiveRemoteConnectionId,
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({ activeFolder: { path: "/repo" } }),
}))

vi.mock("@/contexts/workspace-context", () => ({
  useOptionalWorkspaceActions: () => null,
  useWorkspaceActions: () => ({ openFilePreview: mocks.openFilePreview }),
}))

import { MessageResponse } from "./message"

function fileBadgeButton(container: HTMLElement): HTMLButtonElement {
  const button = container.querySelector<HTMLButtonElement>(
    "button[data-resource-kind='file']"
  )
  if (!button) throw new Error("expected a clickable file badge")
  return button
}

describe("MessageResponse — relative local file links (real Streamdown)", () => {
  beforeEach(() => {
    mocks.openFilePreview.mockReset()
    mocks.openFilePreview.mockResolvedValue(undefined)
    mocks.toastError.mockReset()
    mocks.isDesktop.mockReturnValue(false)
    mocks.getActiveRemoteConnectionId.mockReturnValue(null)
    vi.spyOn(window, "open").mockReturnValue(null)
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it.each([
    ["./index.html", "index.html"],
    ["index.html", "index.html"],
    ["../site/index.html", "../site/index.html"],
  ])(
    "opens %s relative to the folder, not at the filesystem root",
    async (href, opened) => {
      const { container } = render(
        <MessageResponse>{`已创建 [index.html](${href})`}</MessageResponse>
      )

      await waitFor(() => {
        expect(fileBadgeButton(container)).toBeTruthy()
      })
      expect(container.textContent).not.toContain("[blocked]")

      fireEvent.click(fileBadgeButton(container))
      await waitFor(() => {
        expect(mocks.openFilePreview).toHaveBeenCalledWith(opened, {
          line: undefined,
        })
      })
      expect(mocks.toastError).not.toHaveBeenCalled()
    }
  )

  it("does not let raw HTML use the carrier to change a link's target", async () => {
    const { container } = render(
      <MessageResponse>
        {
          '<a href="./a.md" data-codeg-relative-href="javascript:alert(1)">x</a>'
        }
      </MessageResponse>
    )
    await waitFor(() => {
      expect(container.textContent).toContain("x")
    })
    expect(container.innerHTML).not.toContain("javascript:")
    expect(container.innerHTML).not.toContain("data-codeg-relative-href")
  })
})
