import {
  act,
  fireEvent,
  render,
  renderHook,
  screen,
} from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import enMessages from "@/i18n/messages/en.json"
import type { BrowserTabState } from "@/lib/browser/types"

const mocks = vi.hoisted(() => ({
  browserAgentGrant: vi.fn(() => Promise.resolve({}) as Promise<unknown>),
  success: vi.fn(),
  error: vi.fn(),
}))
vi.mock("@/lib/browser/browser-api", () => ({
  browserAgentGrant: mocks.browserAgentGrant,
}))
vi.mock("sonner", () => ({
  toast: { success: mocks.success, error: mocks.error },
}))

import {
  recordBrowserAgentActivity,
  resetBrowserTabStoreForTests,
} from "@/lib/browser/browser-tab-store"

import {
  BrowserAgentShareControl,
  BrowserAgentStrip,
  useBrowserAgentGlow,
} from "./browser-agent-access"

const tab = {
  id: "browser:abc",
  kind: "browser",
  folderId: 1,
  title: "example.com",
  browser: {
    initialUrl: "https://example.com/",
    openerTabId: null,
    profile: "default",
  },
} as BrowserWorkspaceTab

function state(over: Partial<BrowserTabState> = {}): BrowserTabState {
  return {
    tabId: "abc",
    ownerWindow: "main",
    kind: "page",
    surface: "child",
    channel: "native",
    channelError: null,
    url: "https://example.com/",
    requestedUrl: "https://example.com/",
    title: "Example",
    favicon: null,
    loading: false,
    canGoBack: false,
    canGoForward: false,
    origin: "https://example.com",
    zoom: 1,
    error: null,
    remoteHost: null,
    openerTabId: null,
    profile: "default",
    agentGrant: null,
    ...over,
  }
}

function wrap(node: React.ReactNode) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {node}
    </NextIntlClientProvider>
  )
}

function read(
  over: Partial<Parameters<typeof recordBrowserAgentActivity>[0]> = {}
) {
  act(() =>
    recordBrowserAgentActivity({
      tabId: "abc",
      action: "read",
      outcome: "done",
      at: 1_700_000_000_000,
      ...over,
    })
  )
}

// jsdom has no `PointerEvent`; Radix reads `button` off the event.
function fireMouse(target: Element, type: string) {
  fireEvent(
    target,
    new MouseEvent(type, { bubbles: true, cancelable: true, button: 0 })
  )
}

async function openMenu(trigger: Element) {
  await act(async () => {
    fireMouse(trigger, "pointerdown")
    fireMouse(trigger, "pointerup")
    fireMouse(trigger, "click")
    await new Promise((resolve) => setTimeout(resolve, 0))
  })
}

describe("the share control", () => {
  beforeEach(() => {
    resetBrowserTabStoreForTests()
    mocks.browserAgentGrant.mockClear()
    mocks.success.mockClear()
    mocks.error.mockClear()
  })
  afterEach(() => resetBrowserTabStoreForTests())

  it("hands the page over at the read level, and says which site that was", async () => {
    wrap(<BrowserAgentShareControl tab={tab} state={state()} />)
    const button = screen.getByRole("button", { name: "Share with agents" })
    expect(button).not.toBeDisabled()
    await act(async () => {
      fireEvent.click(button)
      await Promise.resolve()
    })
    // Never `control`: nothing in the app can act on a page yet.
    expect(mocks.browserAgentGrant).toHaveBeenCalledWith("abc", "read")
    expect(mocks.success).toHaveBeenCalledWith(
      "Agents can now read example.com"
    )
  })

  // A tab with no web origin cannot be shared at all — the grant is a binding
  // to one site, and there is nothing here to bind to. Refused up front
  // rather than discovered through an error.
  it("is inert on a page with no web address", () => {
    wrap(
      <BrowserAgentShareControl
        tab={tab}
        state={state({ url: "about:blank", origin: null })}
      />
    )
    const button = screen.getByRole("button", { name: "Share with agents" })
    expect(button).toBeDisabled()
    expect(button).toHaveAttribute(
      "title",
      "This page has no web address to share with agents"
    )
  })

  // A document guest shows a local file, and its address does not give that
  // away: WebView2 serves it from `https://codeg-doc.localhost/…`. The
  // backend refuses to bind a grant to one; the control has to agree, or it
  // would offer a share that can only ever fail.
  it("is inert on a document guest, whose address looks like any other site", () => {
    wrap(
      <BrowserAgentShareControl
        tab={tab}
        state={state({
          kind: "document",
          url: "https://codeg-doc.localhost/report.html",
          origin: "https://codeg-doc.localhost",
        })}
      />
    )
    expect(
      screen.getByRole("button", { name: "Share with agents" })
    ).toBeDisabled()
  })

  it("names the shared site and takes it back from the menu", async () => {
    wrap(
      <BrowserAgentShareControl
        tab={tab}
        state={state({
          agentGrant: {
            level: "read",
            origin: "https://example.com",
            grantedAt: 1,
          },
        })}
      />
    )
    const chip = screen.getByRole("button", {
      name: "Agents can read example.com",
    })
    expect(chip).toHaveTextContent("Shared")
    await openMenu(chip)
    expect(
      screen.getByText("Sharing ends as soon as the page leaves this site.")
    ).toBeVisible()
    await act(async () => {
      fireEvent.click(screen.getByRole("menuitem", { name: "Stop sharing" }))
    })
    expect(mocks.browserAgentGrant).toHaveBeenCalledWith("abc", "none")
    // Taking access back is not an event worth congratulating anyone over.
    expect(mocks.success).not.toHaveBeenCalled()
  })

  it("reports a refused share instead of silently doing nothing", async () => {
    mocks.browserAgentGrant.mockImplementationOnce(() =>
      Promise.reject(new Error("no origin"))
    )
    wrap(<BrowserAgentShareControl tab={tab} state={state()} />)
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Share with agents" }))
      await Promise.resolve()
      await Promise.resolve()
    })
    expect(mocks.error).toHaveBeenCalledWith(
      "This page can't be shared with agents",
      expect.objectContaining({
        description: expect.stringContaining("no origin"),
      })
    )
  })
})

describe("the activity strip", () => {
  beforeEach(() => resetBrowserTabStoreForTests())
  afterEach(() => resetBrowserTabStoreForTests())

  it("is absent until an agent has done something", () => {
    const { container } = wrap(<BrowserAgentStrip tab={tab} />)
    expect(container).toBeEmptyDOMElement()
  })

  // The one case nothing else in the app reports: only the agent is told it
  // was refused.
  it("shows a refusal on a tab nobody shared", () => {
    read({ outcome: "refused" })
    wrap(<BrowserAgentStrip tab={tab} />)
    expect(screen.getByText(/Refused: this page isn't shared/)).toBeVisible()
  })

  it("counts a run rather than repeating it, and keeps the older lines behind a disclosure", async () => {
    read({ at: 1_700_000_000_000 })
    read({ at: 1_700_000_001_000 })
    read({ at: 1_700_000_002_000, outcome: "failed" })
    wrap(<BrowserAgentStrip tab={tab} />)
    expect(screen.getByText(/Couldn't read the page/)).toBeVisible()
    // The run of two is one line, and it is not on screen yet.
    expect(screen.queryByText(/Read the page/)).toBeNull()
    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: /1 more/ }))
    })
    expect(screen.getByText(/Read the page/)).toBeVisible()
    expect(screen.getByText("2×")).toBeVisible()
  })
})

describe("the page border", () => {
  beforeEach(() => {
    vi.useFakeTimers()
    resetBrowserTabStoreForTests()
  })
  afterEach(() => {
    vi.useRealTimers()
    resetBrowserTabStoreForTests()
  })

  it("is drawn only on a shared tab, and brightens while an agent is at work", () => {
    const shared = state({
      agentGrant: {
        level: "read",
        origin: "https://example.com",
        grantedAt: 1,
      },
    })
    const view = renderHook(
      ({ tabState }) => useBrowserAgentGlow(tab, tabState),
      { initialProps: { tabState: state() } }
    )
    expect(view.result.current).toBe("none")

    view.rerender({ tabState: shared })
    expect(view.result.current).toBe("steady")

    read({ at: 1 })
    expect(view.result.current).toBe("active")
    act(() => void vi.advanceTimersByTime(1200))
    expect(view.result.current).toBe("steady")

    // A second attempt while the border is still lit restarts the countdown
    // rather than letting the first one's timer end it mid-run.
    read({ at: 2 })
    expect(view.result.current).toBe("active")
    act(() => void vi.advanceTimersByTime(800))
    read({ at: 3 })
    act(() => void vi.advanceTimersByTime(800))
    expect(view.result.current).toBe("active")
    act(() => void vi.advanceTimersByTime(500))
    expect(view.result.current).toBe("steady")

    // Two attempts can land in the same millisecond — a refusal costs no page
    // round trip. Each pushes a fresh line, so both carry `count: 1`, and
    // with the same `at` the pair is indistinguishable by anything except
    // what happened. Both of these are the head at some point, both are
    // `900` / `1`, and the border still has to notice the second.
    act(() => void vi.advanceTimersByTime(2000))
    read({ at: 900, outcome: "refused" })
    act(() => void vi.advanceTimersByTime(2000))
    expect(view.result.current).toBe("steady")
    read({ at: 900, outcome: "done" })
    expect(view.result.current).toBe("active")

    // Nothing is drawn on a tab that is no longer shared, however busy the
    // agent was a moment ago.
    read({ at: 4 })
    view.rerender({ tabState: state() })
    expect(view.result.current).toBe("none")
    view.unmount()
  })
})
