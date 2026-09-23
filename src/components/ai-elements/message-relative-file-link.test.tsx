import { fireEvent, render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

// End-to-end guard for relative local file links. rehype-harden resolves a
// schemeless href against a placeholder origin and keeps only its pathname, so
// `./index.html` used to open `/index.html` at the filesystem root, and a bare
// `index.html` did not parse at all and became "<name> [blocked]". Exercises
// the REAL Streamdown pipeline (no streamdown mock), so the assertions cover
// actual rehype `sanitize` + `harden` behavior and the restore step after it.
// Only the leaf dependencies of the real link-safety hook are stubbed, so the
// click path (badge → link-safety → `openFilePreview`) is genuinely exercised
// too.
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
    ["deploy.sh", "deploy.sh"],
    ["<./my notes.md>", "my notes.md"],
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

  it("keeps a reference link on the definition it resolves to", async () => {
    // CommonMark takes the first `[doc]:`; the relative duplicate flattens to
    // the same `/docs/a.md` through harden, and must not repoint the link.
    const { container } = render(
      <MessageResponse>
        {"see [a][doc]\n\n[doc]: /docs/a.md\n[doc]: docs/a.md"}
      </MessageResponse>
    )
    await waitFor(() => {
      expect(fileBadgeButton(container)).toBeTruthy()
    })

    fireEvent.click(fileBadgeButton(container))
    await waitFor(() => {
      expect(mocks.openFilePreview).toHaveBeenCalledWith("/docs/a.md", {
        line: undefined,
      })
    })
  })

  it("leaves a scheme-less web address alone rather than guess it is a file", async () => {
    const { container } = render(
      <MessageResponse>{"see [the repo](github.com/foo/bar)"}</MessageResponse>
    )
    await waitFor(() => {
      expect(container.textContent).toContain("the repo")
    })
    expect(
      container.querySelector("button[data-resource-kind='file']")
    ).toBeNull()
  })

  it("does not let raw HTML use the carrier to bring in a scheme", async () => {
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
