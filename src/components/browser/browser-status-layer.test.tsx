import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  actions: null as null | { openBrowserTab: ReturnType<typeof vi.fn> },
  openUrl: vi.fn(),
  revealItemInDir: vi.fn(),
}))

vi.mock("@/contexts/workspace-context", () => ({
  useOptionalWorkspaceActions: () => mocks.actions,
}))
vi.mock("@/lib/browser/browser-api", () => ({ browserReload: vi.fn() }))
vi.mock("@/lib/platform", () => ({
  openUrl: mocks.openUrl,
  revealItemInDir: mocks.revealItemInDir,
}))

import {
  BrowserDownloadBar,
  BrowserErrorPage,
  BrowserNoticeBar,
} from "./browser-status-layer"
import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import enMessages from "@/i18n/messages/en.json"
import {
  resetBrowserTabStoreForTests,
  setBrowserTabNotice,
} from "@/lib/browser/browser-tab-store"
import {
  resetBrowserDownloadsForTests,
  setBrowserDownload,
} from "@/lib/browser/browser-downloads-store"
import type { BrowserDownload } from "@/lib/browser/types"

const tab = {
  id: "browser:abc",
  kind: "browser",
  folderId: null,
  title: "example.com",
  description: null,
  path: null,
  language: "browser",
  content: "",
  loading: true,
  readonly: true,
  browser: { initialUrl: "https://example.com/", openerTabId: null },
} as unknown as BrowserWorkspaceTab

function renderBar() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <BrowserNoticeBar tab={tab} state={null} />
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  resetBrowserTabStoreForTests()
  resetBrowserDownloadsForTests()
  mocks.actions = null
  mocks.openUrl.mockClear()
  mocks.revealItemInDir.mockClear()
})

describe("BrowserNoticeBar", () => {
  it("renders nothing without a notice or a remote host", () => {
    const { container } = renderBar()
    expect(container).toBeEmptyDOMElement()
  })

  it("offers to open a blocked pop-up as a tab next to its opener", () => {
    const openBrowserTab = vi.fn()
    mocks.actions = { openBrowserTab }
    renderBar()
    act(() =>
      setBrowserTabNotice(tab.id, {
        kind: "popup-denied",
        url: "https://accounts.example.com/login?x=1",
        reason: "no-gesture",
      })
    )
    expect(
      screen.getByText(/Pop-up blocked: accounts\.example\.com/)
    ).toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Open anyway" }))
    expect(openBrowserTab).toHaveBeenCalledWith(
      "https://accounts.example.com/login?x=1",
      { openerTabId: "browser:abc" }
    )
    // Acting on the notice also dismisses it.
    expect(screen.queryByText(/Pop-up blocked/)).not.toBeInTheDocument()
  })

  it("only reports the block outside the workspace providers", () => {
    renderBar()
    act(() =>
      setBrowserTabNotice(tab.id, {
        kind: "popup-denied",
        url: "https://example.org/",
        reason: "blocked-scheme",
      })
    )
    expect(screen.getByText(/Pop-up blocked/)).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Open anyway" })
    ).not.toBeInTheDocument()
    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }))
    expect(screen.queryByText(/Pop-up blocked/)).not.toBeInTheDocument()
  })
})

function renderError(error: {
  kind: string
  message: string
  url: string | null
}) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <BrowserErrorPage
        tab={tab}
        error={error as never}
        url="https://example.com/"
      />
    </NextIntlClientProvider>
  )
}

describe("BrowserErrorPage", () => {
  it("explains a navigation that never produced a page", () => {
    renderError({
      kind: "failed",
      message: "",
      url: "https://news.example.com/",
    })
    expect(screen.getByText("This page can't be loaded")).toBeInTheDocument()
    expect(screen.getByText("https://news.example.com/")).toBeInTheDocument()
    expect(screen.getByText(/a proxy may be required/)).toBeInTheDocument()
  })

  it("prefers the platform's own wording and the tab URL as a fallback", () => {
    renderError({ kind: "tls", message: "certificate expired", url: null })
    expect(
      screen.getByText("This connection is not secure")
    ).toBeInTheDocument()
    expect(screen.getByText("https://example.com/")).toBeInTheDocument()
    expect(screen.getByText("certificate expired")).toBeInTheDocument()
    expect(
      screen.queryByText(/a proxy may be required/)
    ).not.toBeInTheDocument()
  })
})

describe("BrowserDownloadBar", () => {
  function renderDownloads() {
    return render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <BrowserDownloadBar tab={tab} />
      </NextIntlClientProvider>
    )
  }

  const download = (over: Partial<BrowserDownload> = {}): BrowserDownload => ({
    id: "dl-1",
    tabId: "abc",
    url: "https://example.com/a.bin",
    fileName: "a.bin",
    path: "/Users/dev/Downloads/a.bin",
    state: "started",
    ...over,
  })

  it("renders nothing without downloads", () => {
    const { container } = renderDownloads()
    expect(container).toBeEmptyDOMElement()
  })

  // Never "open": a file that just arrived from the web is revealed in the
  // file manager, and running it stays the user's decision.
  it("offers show-in-folder only once a download completed", () => {
    renderDownloads()
    act(() => setBrowserDownload(download()))
    expect(screen.getByText("a.bin")).toBeInTheDocument()
    expect(screen.getByText("Downloading…")).toBeInTheDocument()
    expect(screen.queryByText("Show in folder")).not.toBeInTheDocument()

    act(() => setBrowserDownload(download({ state: "completed" })))
    expect(screen.getByText("Saved")).toBeInTheDocument()
    fireEvent.click(screen.getByText("Show in folder"))
    expect(mocks.revealItemInDir).toHaveBeenCalledWith(
      "/Users/dev/Downloads/a.bin"
    )
  })

  it("shows a failed download and lets the row be dismissed", () => {
    renderDownloads()
    act(() => setBrowserDownload(download({ state: "failed" })))
    expect(screen.getByText("Failed")).toBeInTheDocument()
    expect(screen.queryByText("Show in folder")).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole("button", { name: "Dismiss" }))
    expect(screen.queryByText("a.bin")).not.toBeInTheDocument()
  })

  it("ignores downloads that belong to another tab", () => {
    const { container } = renderDownloads()
    act(() => setBrowserDownload(download({ tabId: "other" })))
    expect(container).toBeEmptyDOMElement()
  })
})
