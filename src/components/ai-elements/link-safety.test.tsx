import { useState } from "react"
import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import type { LinkSafetyModalProps } from "streamdown"
import {
  FilePathLink,
  useStreamdownLinkSafety,
} from "@/components/ai-elements/link-safety"

const mocks = vi.hoisted(() => ({
  openUrl: vi.fn(),
  openFilePreview: vi.fn(),
  toastError: vi.fn(),
  isDesktop: vi.fn(() => false),
  getActiveRemoteConnectionId: vi.fn(() => null),
  activeFolderPath: "/repo",
}))

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) => key,
}))

vi.mock("sonner", () => ({
  toast: {
    error: mocks.toastError,
  },
}))

vi.mock("@/lib/platform", () => ({
  openUrl: mocks.openUrl,
}))

vi.mock("@/lib/transport", () => ({
  isDesktop: mocks.isDesktop,
  getActiveRemoteConnectionId: mocks.getActiveRemoteConnectionId,
}))

vi.mock("@/contexts/active-folder-context", () => ({
  useActiveFolder: () => ({
    activeFolder: {
      path: mocks.activeFolderPath,
    },
  }),
}))

vi.mock("@/contexts/workspace-context", () => ({
  useWorkspaceContext: () => ({
    openFilePreview: mocks.openFilePreview,
  }),
}))

function LinkSafetyHarness({ url }: { url: string }) {
  const linkSafety = useStreamdownLinkSafety()
  const [open, setOpen] = useState(false)
  const renderModal = linkSafety.renderModal

  const props: LinkSafetyModalProps = {
    url,
    isOpen: open,
    onClose: () => setOpen(false),
    onConfirm: () => {},
  }

  return (
    <div>
      <button
        type="button"
        onClick={async () => {
          if (linkSafety.onLinkCheck && (await linkSafety.onLinkCheck(url))) {
            window.open(url, "_blank", "noreferrer")
            return
          }
          setOpen(true)
        }}
      >
        Trigger link
      </button>
      {renderModal?.(props)}
    </div>
  )
}

describe("link safety direct opening", () => {
  beforeEach(() => {
    mocks.openUrl.mockReset()
    mocks.openFilePreview.mockReset()
    mocks.toastError.mockReset()
    mocks.isDesktop.mockReset()
    mocks.isDesktop.mockReturnValue(false)
    mocks.getActiveRemoteConnectionId.mockReset()
    mocks.getActiveRemoteConnectionId.mockReturnValue(null)
    mocks.openFilePreview.mockResolvedValue(undefined)
    mocks.activeFolderPath = "/repo"
    vi.spyOn(window, "open").mockReturnValue(null)
  })

  afterEach(() => {
    vi.restoreAllMocks()
  })

  it("opens markdown hyperlinks directly from Streamdown without rendering a confirmation dialog", async () => {
    render(<LinkSafetyHarness url="https://example.com/docs" />)

    fireEvent.click(screen.getByRole("button", { name: "Trigger link" }))

    await waitFor(() => {
      expect(window.open).toHaveBeenCalledWith(
        "https://example.com/docs",
        "_blank",
        "noreferrer"
      )
    })
    expect(mocks.openUrl).not.toHaveBeenCalled()
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
  })

  it("opens markdown file links directly in the workspace", async () => {
    render(<LinkSafetyHarness url="file:///repo/src/app.ts#L12" />)

    fireEvent.click(screen.getByRole("button", { name: "Trigger link" }))

    await waitFor(() => {
      expect(mocks.openFilePreview).toHaveBeenCalledWith("src/app.ts", {
        line: 12,
      })
    })
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
  })

  it("blocks unsupported markdown link protocols without rendering a confirmation dialog", async () => {
    render(<LinkSafetyHarness url="vscode://file/repo/src/app.ts" />)

    fireEvent.click(screen.getByRole("button", { name: "Trigger link" }))

    await waitFor(() => {
      expect(mocks.toastError).toHaveBeenCalledWith("errorFailedLink", {
        description: "errorUnsupportedLinkProtocol",
      })
    })
    expect(window.open).not.toHaveBeenCalled()
    expect(mocks.openUrl).not.toHaveBeenCalled()
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
  })

  it("treats protocol-relative // URLs as local paths, not browser links", async () => {
    // `//cdn.example.com/app.js` begins with "/", so parseLocalFileTarget
    // claims it and it routes through the file opener (here: rejected as
    // outside the workspace) rather than window.open. This is why
    // classifyResourceKind tags `//…` with the file icon, not the web icon.
    render(<LinkSafetyHarness url="//cdn.example.com/app.js" />)

    fireEvent.click(screen.getByRole("button", { name: "Trigger link" }))

    await waitFor(() => {
      expect(mocks.toastError).toHaveBeenCalledWith("errorCannotOpen", {
        description: "errorOutsideWorkspace",
      })
    })
    expect(window.open).not.toHaveBeenCalled()
    expect(mocks.openFilePreview).not.toHaveBeenCalled()
  })

  it("opens file path labels directly in the workspace", async () => {
    render(
      <FilePathLink filePath="/repo/src/lib.ts" line={5}>
        src/lib.ts
      </FilePathLink>
    )

    fireEvent.click(screen.getByRole("button", { name: "src/lib.ts" }))

    await waitFor(() => {
      expect(mocks.openFilePreview).toHaveBeenCalledWith("src/lib.ts", {
        line: 5,
      })
    })
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
  })

  it("routes desktop external links through the platform opener instead of streamdown", async () => {
    mocks.isDesktop.mockReturnValue(true)
    mocks.openUrl.mockResolvedValue(undefined)

    render(<LinkSafetyHarness url="https://example.com/docs" />)

    fireEvent.click(screen.getByRole("button", { name: "Trigger link" }))

    await waitFor(() => {
      expect(mocks.openUrl).toHaveBeenCalledWith("https://example.com/docs")
    })
    expect(window.open).not.toHaveBeenCalled()
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
  })

  it("opens mailto: links via a synthetic anchor click in the browser to avoid an about:blank tab", async () => {
    mocks.isDesktop.mockReturnValue(false)
    const clickedHrefs: string[] = []
    const clickSpy = vi
      .spyOn(HTMLElement.prototype, "click")
      .mockImplementation(function (this: HTMLElement) {
        if (this instanceof HTMLAnchorElement) clickedHrefs.push(this.href)
      })

    render(<LinkSafetyHarness url="mailto:hi@example.com" />)

    fireEvent.click(screen.getByRole("button", { name: "Trigger link" }))

    await waitFor(() => {
      expect(clickedHrefs).toContain("mailto:hi@example.com")
    })
    expect(mocks.openUrl).not.toHaveBeenCalled()
    expect(window.open).not.toHaveBeenCalled()
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument()
    clickSpy.mockRestore()
  })

  it("opens mailto: links via the platform opener on desktop", async () => {
    mocks.isDesktop.mockReturnValue(true)
    mocks.openUrl.mockResolvedValue(undefined)

    render(<LinkSafetyHarness url="mailto:hi@example.com" />)

    fireEvent.click(screen.getByRole("button", { name: "Trigger link" }))

    await waitFor(() => {
      expect(mocks.openUrl).toHaveBeenCalledWith("mailto:hi@example.com")
    })
    expect(window.open).not.toHaveBeenCalled()
  })

  it("reflects the in-flight state on the file path button while a preview is opening", async () => {
    let resolveOpen: (() => void) | undefined
    mocks.openFilePreview.mockImplementation(
      () =>
        new Promise<void>((resolve) => {
          resolveOpen = resolve
        })
    )

    render(<FilePathLink filePath="/repo/src/lib.ts">src/lib.ts</FilePathLink>)

    const button = screen.getByRole("button", { name: "src/lib.ts" })
    fireEvent.click(button)

    await waitFor(() => {
      expect(button).toBeDisabled()
      expect(button).toHaveAttribute("aria-busy", "true")
    })

    // Clicking while busy must not enqueue another open call.
    fireEvent.click(button)
    expect(mocks.openFilePreview).toHaveBeenCalledTimes(1)

    await act(async () => {
      resolveOpen?.()
    })

    await waitFor(() => {
      expect(button).not.toBeDisabled()
      expect(button).toHaveAttribute("aria-busy", "false")
    })
  })

  it("survives a parent re-render that swaps handler identities mid-flight", async () => {
    let resolvePendingOpen: (() => void) | undefined
    const initialOpenPreview = vi.fn(
      () =>
        new Promise<void>((resolve) => {
          resolvePendingOpen = resolve
        })
    )
    mocks.openFilePreview = initialOpenPreview

    function ChurningHarness({ url, churn }: { url: string; churn: number }) {
      const linkSafety = useStreamdownLinkSafety()
      const [open, setOpen] = useState(false)
      return (
        <div data-churn={churn}>
          <button type="button" onClick={() => setOpen(true)}>
            Trigger link
          </button>
          {linkSafety.renderModal?.({
            url,
            isOpen: open,
            onClose: () => setOpen(false),
            onConfirm: () => {},
          })}
        </div>
      )
    }

    const { rerender } = render(
      <ChurningHarness url="file:///repo/src/app.ts" churn={0} />
    )

    fireEvent.click(screen.getByRole("button", { name: "Trigger link" }))

    await waitFor(() => {
      expect(initialOpenPreview).toHaveBeenCalledTimes(1)
    })

    // Swap `useWorkspaceContext().openFilePreview` to a fresh vi.fn so the
    // next render forces `useOpenLinkOrFile`'s `useCallback` to rebuild —
    // i.e. the `onAction` prop of `<DirectLinkOpen>` changes identity while
    // the original open is still pending. The previous `cancelled`-flag
    // implementation tore down the in-flight callback chain here and never
    // fired `onClose()`, stranding streamdown's `isOpen` at true.
    const replacementOpenPreview = vi.fn().mockResolvedValue(undefined)
    mocks.openFilePreview = replacementOpenPreview
    rerender(<ChurningHarness url="file:///repo/src/app.ts" churn={1} />)

    await act(async () => {
      resolvePendingOpen?.()
    })

    // A second click must reach the replacement handler — which proves the
    // modal state was reset by `onClose()` despite the mid-flight identity
    // change, and that subsequent opens route through the latest handler.
    fireEvent.click(screen.getByRole("button", { name: "Trigger link" }))

    await waitFor(() => {
      expect(replacementOpenPreview).toHaveBeenCalledTimes(1)
    })
    expect(initialOpenPreview).toHaveBeenCalledTimes(1)
  })
})
