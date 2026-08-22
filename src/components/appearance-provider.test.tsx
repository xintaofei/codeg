import { act, fireEvent, render, screen } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { AppearanceProvider } from "./appearance-provider"
import { useCustomStyle, useSidebarNavVisibility } from "@/hooks/use-appearance"
import {
  STORAGE_KEY_CUSTOM_THEME,
  STORAGE_KEY_SIDEBAR_NAV_VISIBILITY,
} from "@/lib/appearance-script"

function Probe() {
  const { setCustomThemeToken } = useCustomStyle()
  return (
    <>
      <button onClick={() => setCustomThemeToken("primary", "#bbbbbb")}>
        set-b
      </button>
      <button onClick={() => setCustomThemeToken("primary", "#aaaaaa")}>
        set-a
      </button>
    </>
  )
}

function renderProbe() {
  return render(
    <AppearanceProvider>
      <Probe />
    </AppearanceProvider>
  )
}

function storedPrimary(): string | undefined {
  const raw = localStorage.getItem(STORAGE_KEY_CUSTOM_THEME)
  return raw ? JSON.parse(raw).light?.primary : undefined
}

beforeEach(() => {
  localStorage.clear()
  document.documentElement.removeAttribute("style")
  document.documentElement.classList.remove("dark")
  localStorage.setItem(
    STORAGE_KEY_CUSTOM_THEME,
    JSON.stringify({ light: { primary: "#aaaaaa" }, dark: {} })
  )
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
  document.documentElement.removeAttribute("style")
})

describe("debounced persistence", () => {
  it("does not resurrect a value that was reverted inside the debounce window", () => {
    // A → B → A，全程不超过防抖窗口，然后关窗触发 flush。此前 flush 会把已经被
    // 撤销的 B 写进去，重载后「撤销」凭空失效，还会经 storage 事件传染给其它窗口。
    renderProbe()
    expect(storedPrimary()).toBe("#aaaaaa")

    fireEvent.click(screen.getByText("set-b"))
    fireEvent.click(screen.getByText("set-a"))

    act(() => {
      window.dispatchEvent(new Event("pagehide"))
    })

    expect(storedPrimary()).toBe("#aaaaaa")
    expect(document.documentElement.style.getPropertyValue("--primary")).toBe(
      "#aaaaaa"
    )
  })

  it("still flushes a genuinely pending edit when the window goes away", () => {
    // 反向对照：撤销的不写，没撤销的必须写 —— 否则「改完随手关窗」会静默丢修改。
    renderProbe()

    fireEvent.click(screen.getByText("set-b"))
    expect(storedPrimary()).toBe("#aaaaaa") // 还没到期，尚未落盘

    act(() => {
      window.dispatchEvent(new Event("pagehide"))
    })

    expect(storedPrimary()).toBe("#bbbbbb")
  })

  it("writes once the debounce elapses, without needing the flush", () => {
    renderProbe()

    fireEvent.click(screen.getByText("set-b"))
    act(() => {
      vi.advanceTimersByTime(500)
    })

    expect(storedPrimary()).toBe("#bbbbbb")
  })
})

function NavVisibilityProbe() {
  const { sidebarNavVisibility, setSidebarNavItemVisible } =
    useSidebarNavVisibility()
  return (
    <>
      <span data-testid="nav-visibility">
        {JSON.stringify(sidebarNavVisibility)}
      </span>
      <button onClick={() => setSidebarNavItemVisible("forge", false)}>
        hide-forge
      </button>
    </>
  )
}

describe("sidebar nav visibility", () => {
  const renderNavProbe = () =>
    render(
      <AppearanceProvider>
        <NavVisibilityProbe />
      </AppearanceProvider>
    )

  const visibility = () =>
    JSON.parse(screen.getByTestId("nav-visibility").textContent ?? "")

  it("defaults every row to visible and round-trips a hide through storage", () => {
    const { unmount } = renderNavProbe()
    expect(visibility()).toEqual({
      automations: true,
      tasks: true,
      forge: true,
    })

    fireEvent.click(screen.getByText("hide-forge"))
    expect(visibility()).toEqual({
      automations: true,
      tasks: true,
      forge: false,
    })
    // Persisted immediately (a discrete toggle, so no debounce to wait out).
    expect(
      JSON.parse(localStorage.getItem(STORAGE_KEY_SIDEBAR_NAV_VISIBILITY) ?? "")
    ).toEqual({ automations: true, tasks: true, forge: false })

    // A fresh mount reads the stored record back: the mount effect reconciles
    // the SSR-safe all-visible initial state with the persisted choice.
    unmount()
    renderNavProbe()
    expect(visibility()).toEqual({
      automations: true,
      tasks: true,
      forge: false,
    })
  })

  it("follows a change written by another window via the storage event", () => {
    // Settings is its own window — without this sync a row hidden there would
    // only disappear from the workspace after a reload.
    renderNavProbe()
    const next = JSON.stringify({
      automations: false,
      tasks: true,
      forge: true,
    })
    act(() => {
      localStorage.setItem(STORAGE_KEY_SIDEBAR_NAV_VISIBILITY, next)
      window.dispatchEvent(
        new StorageEvent("storage", {
          key: STORAGE_KEY_SIDEBAR_NAV_VISIBILITY,
          newValue: next,
        })
      )
    })
    expect(visibility()).toEqual({
      automations: false,
      tasks: true,
      forge: true,
    })
  })
})
