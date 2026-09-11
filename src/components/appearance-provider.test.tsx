import { act, fireEvent, render, screen } from "@testing-library/react"
import { memo, useEffect } from "react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { AppearanceProvider } from "./appearance-provider"
import {
  useAppearancePresets,
  useCodeTheme,
  useCustomStyle,
  useMonoFont,
} from "@/hooks/use-appearance"
import {
  STORAGE_KEY_APPEARANCE_PRESET,
  STORAGE_KEY_CODE_THEME,
  STORAGE_KEY_CUSTOM_THEME,
  STORAGE_KEY_CUSTOM_THEME_ENABLED,
  STORAGE_KEY_MONO_FONT,
  STORAGE_KEY_MONO_FONT_STACK,
  STORAGE_KEY_THEME_COLOR,
  STORAGE_KEY_UI_FONT,
  STORAGE_KEY_ZOOM_LEVEL,
} from "@/lib/appearance-script"
import { BUNDLED_PRESET_BY_ID } from "@/lib/appearance-presets-bundled"
import { presetToApplication } from "@/lib/appearance-preset"

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

describe("appearance presets", () => {
  const warmTerminal = BUNDLED_PRESET_BY_ID["warm-terminal"]
  const denseIde = BUNDLED_PRESET_BY_ID["dense-ide"]

  function PresetProbe() {
    const { applyPreset, appliedPreset, codeTheme } = useAppearancePresets()
    const { monoFont } = useMonoFont()
    return (
      <>
        <button onClick={() => applyPreset(warmTerminal)}>apply-warm</button>
        <button onClick={() => applyPreset(denseIde)}>apply-dense</button>
        <span data-testid="applied">{appliedPreset?.id ?? "none"}</span>
        <span data-testid="code">
          {codeTheme.light}/{codeTheme.dark}
        </span>
        <span data-testid="mono">{monoFont.id}</span>
      </>
    )
  }

  function rootVar(name: string) {
    return document.documentElement.style.getPropertyValue(name)
  }

  beforeEach(() => {
    localStorage.clear()
    document.documentElement.removeAttribute("data-theme")
  })

  it("applies a preset through the same keys a hand edit writes, before and after a reload", () => {
    render(
      <AppearanceProvider>
        <PresetProbe />
      </AppearanceProvider>
    )

    fireEvent.click(screen.getByText("apply-warm"))
    act(() => {
      vi.advanceTimersByTime(500) // the custom-theme debounce
    })

    const plan = presetToApplication(warmTerminal)
    // Live DOM: base palette, both kinds of token, fonts.
    expect(document.documentElement.getAttribute("data-theme")).toBe("stone")
    expect(rootVar("--primary")).toBe(plan.customTheme.light.primary)
    expect(rootVar("--status-bar-bg")).toBe(
      plan.customTheme.light["status-bar-bg"]
    )
    expect(rootVar("--spacing")).toBe("0.2375rem")
    expect(rootVar("--chat-font-size")).toBe("0.8125rem")
    expect(rootVar("--font-sans")).toContain("JetBrains Mono")
    expect(rootVar("--font-mono")).toContain("JetBrains Mono")
    expect(screen.getByTestId("code").textContent).toBe(
      "vitesse-light/vitesse-dark"
    )
    expect(screen.getByTestId("applied").textContent).toBe("warm-terminal")

    // Storage: exactly what the pre-paint script and a fresh provider read.
    expect(localStorage.getItem(STORAGE_KEY_THEME_COLOR)).toBe("stone")
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY_CUSTOM_THEME)!)).toEqual(
      plan.customTheme
    )
    expect(localStorage.getItem(STORAGE_KEY_CUSTOM_THEME_ENABLED)).toBe("1")
    expect(localStorage.getItem(STORAGE_KEY_UI_FONT)).toBe("jetbrains-mono")
    expect(localStorage.getItem(STORAGE_KEY_MONO_FONT)).toBe("jetbrains-mono")
    expect(localStorage.getItem(STORAGE_KEY_MONO_FONT_STACK)).toContain(
      "JetBrains Mono"
    )
    expect(JSON.parse(localStorage.getItem(STORAGE_KEY_CODE_THEME)!)).toEqual({
      light: "vitesse-light",
      dark: "vitesse-dark",
    })
    expect(
      JSON.parse(localStorage.getItem(STORAGE_KEY_APPEARANCE_PRESET)!).id
    ).toBe("warm-terminal")
  })

  it("re-enables custom colours when a preset is applied", () => {
    localStorage.setItem(STORAGE_KEY_CUSTOM_THEME_ENABLED, "0")
    render(
      <AppearanceProvider>
        <PresetProbe />
      </AppearanceProvider>
    )
    expect(rootVar("--primary")).toBe("")

    fireEvent.click(screen.getByText("apply-dense"))

    expect(rootVar("--primary")).toBe(
      presetToApplication(denseIde).customTheme.light.primary
    )
    expect(localStorage.getItem(STORAGE_KEY_CUSTOM_THEME_ENABLED)).toBe("1")
  })

  it("follows a preset applied in another window", () => {
    render(
      <AppearanceProvider>
        <PresetProbe />
      </AppearanceProvider>
    )
    const plan = presetToApplication(denseIde)

    act(() => {
      for (const [key, value] of [
        [STORAGE_KEY_THEME_COLOR, "slate"],
        [STORAGE_KEY_CUSTOM_THEME, JSON.stringify(plan.customTheme)],
        [STORAGE_KEY_MONO_FONT, "jetbrains-mono"],
        [STORAGE_KEY_CODE_THEME, JSON.stringify(plan.codeTheme)],
        [STORAGE_KEY_APPEARANCE_PRESET, JSON.stringify(denseIde)],
      ] as const) {
        localStorage.setItem(key, value)
        window.dispatchEvent(
          new StorageEvent("storage", { key, newValue: value })
        )
      }
    })

    expect(document.documentElement.getAttribute("data-theme")).toBe("slate")
    expect(rootVar("--spacing")).toBe("0.2125rem")
    expect(rootVar("--font-mono")).toContain("JetBrains Mono")
    expect(screen.getByTestId("mono").textContent).toBe("jetbrains-mono")
    expect(screen.getByTestId("code").textContent).toBe("light-plus/dark-plus")
    expect(screen.getByTestId("applied").textContent).toBe("dense-ide")
  })

  it("does not re-render messages for a preset that keeps the code colours", () => {
    // A transcript reads only the code-theme context. Everything else a
    // preset changes is a CSS variable, so switching presets must cost the
    // message list nothing; only a change of Shiki themes may re-render it,
    // because the tokens really are different then.
    // Counted from an effect (one commit per render here: no StrictMode, no
    // suspense), which keeps the probe's render body pure.
    const renders = { count: 0 }
    const Message = memo(function Message() {
      const [light, dark] = useCodeTheme()
      useEffect(() => {
        renders.count += 1
      })
      return (
        <span data-testid="message">
          {light}/{dark}
        </span>
      )
    })
    // Apply "ink" first, then "high-contrast": both change palette, fonts,
    // spacing and bubbles, and only the second changes the code theme.
    const ink = BUNDLED_PRESET_BY_ID["ink"]
    const highContrast = BUNDLED_PRESET_BY_ID["high-contrast"]
    function Controls() {
      const { applyPreset } = useAppearancePresets()
      return (
        <>
          <button onClick={() => applyPreset(ink)}>ink</button>
          <button onClick={() => applyPreset(highContrast)}>hc</button>
          <button onClick={() => applyPreset(warmTerminal)}>warm</button>
        </>
      )
    }
    render(
      <AppearanceProvider>
        <Controls />
        <Message />
        <Message />
      </AppearanceProvider>
    )
    expect(renders.count).toBe(2)

    fireEvent.click(screen.getByText("ink"))
    // ink sets min-light / min-dark: a real change, one render per message.
    expect(renders.count).toBe(4)
    expect(screen.getAllByTestId("message")[0].textContent).toBe(
      "min-light/min-dark"
    )

    // Re-applying ink writes every token again and keeps the code theme: no
    // message renders.
    fireEvent.click(screen.getByText("ink"))
    expect(renders.count).toBe(4)

    // Presets that do change the code theme render each message once.
    fireEvent.click(screen.getByText("hc"))
    expect(renders.count).toBe(6)
    fireEvent.click(screen.getByText("warm"))
    expect(renders.count).toBe(8)
    // And a zoom change, which rewrites the appearance context, costs nothing.
    act(() => {
      window.dispatchEvent(
        new KeyboardEvent("keydown", {
          key: "=",
          ctrlKey: true,
          bubbles: true,
          cancelable: true,
        })
      )
    })
    expect(renders.count).toBe(8)
  })

  it("writes a shared token to both modes and clears it from both", () => {
    function SharedProbe() {
      const { setSharedThemeToken, customTheme } = useAppearancePresets()
      return (
        <>
          <button onClick={() => setSharedThemeToken("spacing", "0.2125rem")}>
            set
          </button>
          <button onClick={() => setSharedThemeToken("spacing", null)}>
            clear
          </button>
          <span data-testid="both">
            {customTheme.light.spacing ?? "-"}/{customTheme.dark.spacing ?? "-"}
          </span>
        </>
      )
    }
    render(
      <AppearanceProvider>
        <SharedProbe />
      </AppearanceProvider>
    )

    fireEvent.click(screen.getByText("set"))
    expect(screen.getByTestId("both").textContent).toBe("0.2125rem/0.2125rem")
    expect(rootVar("--spacing")).toBe("0.2125rem")

    fireEvent.click(screen.getByText("clear"))
    expect(screen.getByTestId("both").textContent).toBe("-/-")
    expect(rootVar("--spacing")).toBe("")
  })
})

describe("window zoom keys", () => {
  function startAt(zoom: number) {
    document.documentElement.style.fontSize = `${(16 * zoom) / 100}px`
  }

  function currentZoomPx(): string {
    return document.documentElement.style.fontSize
  }

  function keydown(
    key: string,
    init: KeyboardEventInit = {},
    target: EventTarget = window
  ) {
    target.dispatchEvent(
      new KeyboardEvent("keydown", {
        key,
        ctrlKey: true,
        bubbles: true,
        cancelable: true,
        ...init,
      })
    )
  }

  /** Zoom writes reach Tauri IPC and an on-disk SQLite upsert on the same path. */
  function zoomWrites(spy: ReturnType<typeof vi.spyOn>): number {
    return spy.mock.calls.filter(([key]) => key === STORAGE_KEY_ZOOM_LEVEL)
      .length
  }

  function renderZoom() {
    render(
      <AppearanceProvider>
        <div data-terminal-panel-region="true">
          <span data-testid="terminal-child">term</span>
        </div>
      </AppearanceProvider>
    )
    return vi.spyOn(Storage.prototype, "setItem")
  }

  it("stops writing once a held zoom-out key hits the bottom of the range", () => {
    // 80% is the lowest rung, so every further repeat is a no-op on screen. It
    // used to persist and hit the DB once per repeat regardless.
    startAt(80)
    const setItem = renderZoom()

    act(() => {
      for (let i = 0; i < 5; i += 1) keydown("-", { repeat: i > 0 })
    })

    expect(zoomWrites(setItem)).toBe(0)
    expect(currentZoomPx()).toBe("12.8px")
  })

  it("never writes when reset is held at 100%", () => {
    // stepZoom clamping does not cover reset: it sets the default outright.
    startAt(100)
    const setItem = renderZoom()

    act(() => {
      for (let i = 0; i < 5; i += 1) keydown("0", { repeat: i > 0 })
    })

    expect(zoomWrites(setItem)).toBe(0)
    expect(currentZoomPx()).toBe("16px")
  })

  it("walks one rung per repeat without dropping a step", () => {
    // All three land in one act(), so no passive effect gets to run between
    // them. Reading the level from an effect-synced ref would step 100 → 110
    // three times over and lose two rungs.
    startAt(100)
    const setItem = renderZoom()

    act(() => {
      keydown("=")
      keydown("=", { repeat: true })
      keydown("=", { repeat: true })
    })

    expect(currentZoomPx()).toBe("24px") // 150%
    expect(zoomWrites(setItem)).toBe(3)
  })

  it("preventDefaults a repeat so the webview does not also page-zoom", () => {
    startAt(100)
    renderZoom()

    const held = new KeyboardEvent("keydown", {
      key: "=",
      ctrlKey: true,
      repeat: true,
      bubbles: true,
      cancelable: true,
    })
    act(() => {
      window.dispatchEvent(held)
    })

    expect(held.defaultPrevented).toBe(true)
  })

  it("declines Ctrl over the terminal but still zooms for Cmd", () => {
    // Ctrl+- and Ctrl+= mean something to the shell; Cmd+- does not, so macOS
    // keeps zooming over the terminal.
    startAt(100)
    const setItem = renderZoom()
    const terminalChild = screen.getByTestId("terminal-child")

    act(() => {
      keydown("=", {}, terminalChild)
    })
    expect(zoomWrites(setItem)).toBe(0)
    expect(currentZoomPx()).toBe("16px")

    act(() => {
      keydown("=", { ctrlKey: false, metaKey: true }, terminalChild)
    })
    expect(currentZoomPx()).toBe("17.6px") // 110%
  })

  it("reaches reset from the unshifted AZERTY zero key", () => {
    startAt(125)
    renderZoom()

    act(() => {
      keydown("à", { code: "Digit0" })
    })

    expect(currentZoomPx()).toBe("16px")
  })
})
