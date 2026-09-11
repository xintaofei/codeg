import { act, fireEvent, render, screen, within } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
}))
vi.mock("sonner", () => ({ toast: toastMock }))

const setThemeMock = vi.hoisted(() => vi.fn())
vi.mock("next-themes", () => ({
  useTheme: () => ({ theme: "light", setTheme: setThemeMock }),
}))

const saveTextFileMock = vi.hoisted(() => vi.fn())
vi.mock("@/lib/save-file", () => ({ saveTextFile: saveTextFileMock }))

import { AppearancePresetsSection } from "./appearance-presets-section"
import { AppearanceProvider } from "@/components/appearance-provider"
import enMessages from "@/i18n/messages/en.json"
import {
  MAX_PRESET_BYTES,
  parseAppearancePreset,
  presetToApplication,
  serializeAppearancePreset,
  type AppearancePreset,
} from "@/lib/appearance-preset"
import { BUNDLED_PRESET_BY_ID } from "@/lib/appearance-presets-bundled"
import {
  STORAGE_KEY_CUSTOM_THEME,
  STORAGE_KEY_THEME_COLOR,
} from "@/lib/appearance-script"

function renderSection() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <AppearanceProvider>
        <AppearancePresetsSection />
      </AppearanceProvider>
    </NextIntlClientProvider>
  )
}

function rootVar(name: string) {
  return document.documentElement.style.getPropertyValue(name)
}

function chooseFile(text: string, name = "look.codeg-preset.json") {
  const input = screen.getByTestId("preset-file-input") as HTMLInputElement
  const file = new File([text], name, { type: "application/json" })
  fireEvent.change(input, { target: { files: [file] } })
}

const imported: AppearancePreset = {
  schemaVersion: 1,
  id: "shared-look",
  name: "Shared look",
  description: "Came in from a file.",
  base: "rose",
  mode: "dark",
  colors: { light: { primary: "#336699" }, dark: { primary: "#99ccff" } },
  density: { spacing: 0.92 },
  code: { light: "one-light", dark: "one-dark-pro" },
}

beforeEach(() => {
  localStorage.clear()
  document.documentElement.removeAttribute("style")
  document.documentElement.removeAttribute("data-theme")
  document.documentElement.classList.remove("dark")
  saveTextFileMock.mockReset()
  saveTextFileMock.mockResolvedValue("saved")
  setThemeMock.mockReset()
  toastMock.success.mockReset()
  toastMock.error.mockReset()
})

afterEach(() => {
  document.documentElement.removeAttribute("style")
})

describe("AppearancePresetsSection", () => {
  it("lights the stock look until a preset is applied, then the applied one", () => {
    renderSection()

    const defaultCard = screen.getByRole("button", { name: "Apply Default" })
    const warmCard = screen.getByRole("button", {
      name: "Apply Warm terminal",
    })
    expect(defaultCard).toHaveAttribute("aria-pressed", "true")
    expect(warmCard).toHaveAttribute("aria-pressed", "false")
    expect(screen.getByText("Current: Default")).toBeInTheDocument()

    fireEvent.click(warmCard)

    const plan = presetToApplication(BUNDLED_PRESET_BY_ID["warm-terminal"])
    expect(document.documentElement.getAttribute("data-theme")).toBe("stone")
    expect(rootVar("--primary")).toBe(plan.customTheme.light.primary)
    expect(rootVar("--spacing")).toBe("0.2375rem")
    expect(warmCard).toHaveAttribute("aria-pressed", "true")
    expect(defaultCard).toHaveAttribute("aria-pressed", "false")
    expect(screen.getByText("Current: Warm terminal")).toBeInTheDocument()
    // The preset asks for dark mode; that goes through next-themes.
    expect(setThemeMock).toHaveBeenCalledWith("dark")
    expect(toastMock.success).toHaveBeenCalledWith(
      "Preset applied: Warm terminal"
    )
  })

  it("reports a tweaked preset as modified rather than pretending it still matches", () => {
    renderSection()
    fireEvent.click(screen.getByRole("button", { name: "Apply Ink" }))
    expect(screen.getByText("Current: Ink")).toBeInTheDocument()

    // A hand edit in another window (or the Custom Style section) changes a
    // token the preset set.
    const stored = presetToApplication(BUNDLED_PRESET_BY_ID["ink"]).customTheme
    const edited = JSON.stringify({
      ...stored,
      light: { ...stored.light, primary: "#ff0000" },
    })
    act(() => {
      localStorage.setItem(STORAGE_KEY_CUSTOM_THEME, edited)
      window.dispatchEvent(
        new StorageEvent("storage", {
          key: STORAGE_KEY_CUSTOM_THEME,
          newValue: edited,
        })
      )
    })

    expect(screen.getByText("Current: Ink (modified)")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Apply Ink" })).toHaveAttribute(
      "aria-pressed",
      "false"
    )
  })

  it("imports a valid file through a preview, and applies it on confirm", async () => {
    renderSection()

    chooseFile(serializeAppearancePreset(imported))

    const dialog = await screen.findByRole("dialog")
    expect(within(dialog).getByText("Shared look")).toBeInTheDocument()
    expect(within(dialog).getByText("Came in from a file.")).toBeInTheDocument()
    expect(within(dialog).getByText("Rose")).toBeInTheDocument()
    expect(within(dialog).getByText("1 light, 1 dark")).toBeInTheDocument()
    expect(
      within(dialog).getByText("One Light / One Dark Pro")
    ).toBeInTheDocument()
    expect(within(dialog).getByText("92%")).toBeInTheDocument()
    // Nothing applied yet: previewing is not applying.
    expect(document.documentElement.getAttribute("data-theme")).toBeNull()

    fireEvent.click(
      within(dialog).getByRole("button", { name: "Apply preset" })
    )

    expect(document.documentElement.getAttribute("data-theme")).toBe("rose")
    expect(rootVar("--primary")).toBe("#336699")
    expect(rootVar("--spacing")).toBe("0.23rem")
    expect(localStorage.getItem(STORAGE_KEY_THEME_COLOR)).toBe("rose")
    expect(setThemeMock).toHaveBeenCalledWith("dark")
    expect(screen.queryByRole("dialog")).toBeNull()
    expect(screen.getByText("Current: Shared look")).toBeInTheDocument()
  })

  it("refuses a bad file with the field that is wrong, and changes nothing", async () => {
    renderSection()

    chooseFile(
      JSON.stringify({
        ...imported,
        colors: { dark: { primry: "#000000", primary: "plum" } },
        density: { spacing: 4 },
        extra: true,
      })
    )

    const dialog = await screen.findByRole("dialog")
    expect(
      within(dialog).getByText("This file cannot be applied")
    ).toBeInTheDocument()
    const items = within(dialog)
      .getAllByRole("listitem")
      .map((li) => li.textContent)
    expect(items).toEqual([
      "extra: Unknown field",
      "colors.dark.primry: Not a theme token",
      "colors.dark.primary: Not a color (use hex, rgb, hsl or oklch)",
      "density.spacing: Out of range (0.75 to 1.25)",
    ])
    expect(document.documentElement.getAttribute("data-theme")).toBeNull()
    expect(rootVar("--primary")).toBe("")
    expect(
      within(dialog).queryByRole("button", { name: "Apply preset" })
    ).toBeNull()
  })

  it("refuses a file over the size cap without reading it", async () => {
    renderSection()

    chooseFile("x".repeat(MAX_PRESET_BYTES + 1))

    const dialog = await screen.findByRole("dialog")
    expect(
      within(dialog).getByText("File is too large (limit 32 KB)")
    ).toBeInTheDocument()
  })

  it("exports the current look as a file that imports back to the same state", async () => {
    renderSection()
    fireEvent.click(screen.getByRole("button", { name: "Apply Midnight" }))

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Export preset…" }))
    })

    expect(saveTextFileMock).toHaveBeenCalledTimes(1)
    const call = saveTextFileMock.mock.calls[0][0]
    expect(call.suggestedName).toBe("midnight.codeg-preset.json")
    expect(call.mimeType).toBe("application/json")
    const parsed = parseAppearancePreset(call.content)
    expect(parsed.ok).toBe(true)
    if (!parsed.ok) return
    // Exact round trip of the bundled document, plus the window's mode.
    expect(parsed.preset).toEqual({
      ...BUNDLED_PRESET_BY_ID["midnight"],
      mode: "light",
    })
    expect(toastMock.success).toHaveBeenCalledWith("Preset saved", {
      description: undefined,
    })
  })

  it("never exports custom CSS", async () => {
    // The security boundary in one assertion: whatever is in the custom CSS
    // store, the preset file does not contain it.
    localStorage.setItem("codeg-custom-css", ".secret { color: red }")
    localStorage.setItem("codeg-custom-css-enabled", "1")
    renderSection()

    await act(async () => {
      fireEvent.click(screen.getByRole("button", { name: "Export preset…" }))
    })

    const content: string = saveTextFileMock.mock.calls[0][0].content
    expect(content).not.toContain("secret")
    expect(content).not.toContain("css")
  })

  it("drives density and message text through the shared tokens", () => {
    renderSection()
    expect(rootVar("--spacing")).toBe("")

    // Radix Select in jsdom: pointer events are simulated through keyboard
    // navigation on the trigger, which the existing tests also rely on.
    const density = screen.getByRole("combobox", { name: "Density" })
    fireEvent.keyDown(density, { key: "ArrowDown" })
    fireEvent.click(screen.getByRole("option", { name: "Compact" }))
    expect(rootVar("--spacing")).toBe("0.2125rem")

    const chatText = screen.getByRole("combobox", { name: "Message text" })
    fireEvent.keyDown(chatText, { key: "ArrowDown" })
    fireEvent.click(screen.getByRole("option", { name: "Large" }))
    expect(rootVar("--chat-font-size")).toBe("0.9375rem")

    // Back to the defaults clears the tokens instead of pinning stock values.
    fireEvent.keyDown(screen.getByRole("combobox", { name: "Density" }), {
      key: "ArrowDown",
    })
    fireEvent.click(screen.getByRole("option", { name: "Default" }))
    expect(rootVar("--spacing")).toBe("")
  })
})
