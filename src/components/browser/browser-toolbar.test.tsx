import { describe, expect, it } from "vitest"

import { normalizeTypedAddress } from "./browser-toolbar"

describe("normalizeTypedAddress", () => {
  it("keeps full http(s) URLs and normalizes them", () => {
    expect(normalizeTypedAddress("  https://Example.com/a b ")).toBe(
      "https://example.com/a%20b"
    )
    expect(normalizeTypedAddress("http://localhost:3000")).toBe(
      "http://localhost:3000/"
    )
  })

  it("adds a scheme to bare hosts: http for local, https otherwise", () => {
    expect(normalizeTypedAddress("localhost:3000/app")).toBe(
      "http://localhost:3000/app"
    )
    expect(normalizeTypedAddress("127.0.0.1:8080")).toBe(
      "http://127.0.0.1:8080/"
    )
    expect(normalizeTypedAddress("192.168.1.5")).toBe("http://192.168.1.5/")
    expect(normalizeTypedAddress("example.com/docs?x=1")).toBe(
      "https://example.com/docs?x=1"
    )
  })

  it("refuses other schemes, words and blanks (no search fallback)", () => {
    expect(normalizeTypedAddress("javascript:alert(1)")).toBeNull()
    expect(normalizeTypedAddress("file:///etc/hosts")).toBeNull()
    expect(normalizeTypedAddress("hello world")).toBeNull()
    expect(normalizeTypedAddress("notes")).toBeNull()
    expect(normalizeTypedAddress("")).toBeNull()
  })
})

// ---------------------------------------------------------------------------
// The profile chip and its menu
// ---------------------------------------------------------------------------

import { act, fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, vi } from "vitest"

import type { BrowserWorkspaceTab } from "@/contexts/workspace-context"
import enMessages from "@/i18n/messages/en.json"
import {
  resetBrowserPrefsForTests,
  setBrowserProfiles,
} from "@/lib/browser/browser-prefs"

import { BrowserToolbar } from "./browser-toolbar"

const toolbarMocks = vi.hoisted(() => ({
  openBrowserTab: vi.fn(() => "browser:new"),
  workspaceActions: null as null | {
    openBrowserTab: (...a: unknown[]) => unknown
  },
}))

vi.mock("@/contexts/workspace-context", () => ({
  useOptionalWorkspaceActions: () => toolbarMocks.workspaceActions,
}))
vi.mock("@/lib/browser/browser-api", () => ({
  // The toolbar also carries the agent share control.
  browserAgentGrant: vi.fn(() => Promise.resolve()),
  browserGoBack: vi.fn(),
  browserGoForward: vi.fn(),
  browserNavigate: vi.fn(),
  browserReload: vi.fn(),
  browserStop: vi.fn(),
}))
vi.mock("@/lib/platform", () => ({ openUrl: vi.fn() }))

function tabIn(profile: string): BrowserWorkspaceTab {
  return {
    id: "browser:abc",
    kind: "browser",
    folderId: 1,
    title: "example.com",
    description: null,
    path: null,
    language: "browser",
    content: "",
    loading: false,
    readonly: true,
    browser: {
      initialUrl: "https://example.com/",
      openerTabId: null,
      profile,
    },
  } as BrowserWorkspaceTab
}

function renderToolbar(profile: string) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <BrowserToolbar tab={tabIn(profile)} state={null} />
    </NextIntlClientProvider>
  )
}

// jsdom has no `PointerEvent`; Radix reads `button` off the event, so a real
// `MouseEvent` under the pointer-event name is what opens the menu.
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

describe("BrowserToolbar profile chip", () => {
  beforeEach(() => {
    resetBrowserPrefsForTests()
    toolbarMocks.openBrowserTab.mockClear()
    toolbarMocks.workspaceActions = {
      openBrowserTab: toolbarMocks.openBrowserTab,
    }
  })

  it("stays out of the way while only the default profile exists", () => {
    renderToolbar("default")
    expect(screen.queryByRole("button", { name: /^Profile:/ })).toBeNull()
  })

  it("names the tab's profile and opens the page in another one from the menu", async () => {
    setBrowserProfiles([{ id: "p-work", name: "Work" }])
    renderToolbar("p-work")
    const chip = screen.getByRole("button", { name: "Profile: Work" })
    expect(chip).toHaveTextContent("Work")

    await openMenu(chip)
    expect(screen.getByText("Open this page in another profile")).toBeVisible()
    const items = screen.getAllByRole("menuitemradio")
    expect(items.map((item) => item.textContent)).toEqual(["Default", "Work"])
    expect(items[1]).toHaveAttribute("aria-checked", "true")

    await act(async () => {
      fireEvent.click(items[0])
    })
    expect(toolbarMocks.openBrowserTab).toHaveBeenCalledWith(
      "https://example.com/",
      { profile: "default", openerTabId: "browser:abc" }
    )
  })

  it("shows the chip for a tab whose profile is gone, and cannot open elsewhere without a workspace", async () => {
    toolbarMocks.workspaceActions = null
    renderToolbar("p-gone")
    const chip = screen.getByRole("button", { name: "Profile: p-gone" })
    await openMenu(chip)
    const items = screen.getAllByRole("menuitemradio")
    expect(items.map((item) => item.textContent)).toEqual(["Default", "p-gone"])
    // Nothing to open a tab with: the other profiles are inert.
    expect(items[0]).toHaveAttribute("aria-disabled", "true")
    await act(async () => {
      fireEvent.click(items[0])
    })
    expect(toolbarMocks.openBrowserTab).not.toHaveBeenCalled()
  })
})
