import { createHash } from "node:crypto"
import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const toastMock = vi.hoisted(() => ({
  success: vi.fn(),
  error: vi.fn(),
}))
vi.mock("sonner", () => ({ toast: toastMock }))

const themeMock = vi.hoisted(() => ({
  theme: "light" as string | undefined,
  setTheme: vi.fn(),
}))
vi.mock("next-themes", () => ({
  useTheme: () => ({ theme: themeMock.theme, setTheme: themeMock.setTheme }),
}))

// Only the transport is stubbed. The index validator, the preset validator,
// the digest comparison and the storage all run for real.
const callMock = vi.hoisted(() => vi.fn())
vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: callMock }),
}))

import { AppearancePresetGallerySection } from "./appearance-preset-gallery-section"
import { AppearanceProvider } from "@/components/appearance-provider"
import enMessages from "@/i18n/messages/en.json"
import {
  serializeAppearancePreset,
  type AppearancePreset,
} from "@/lib/appearance-preset"
import {
  DEFAULT_PRESET_GALLERY_INDEX_URL,
  MAX_GALLERY_INDEX_BYTES,
  type GalleryDocument,
} from "@/lib/appearance-preset-gallery"
import {
  STORAGE_KEY_APPEARANCE_PRESET,
  STORAGE_KEY_CUSTOM_THEME,
  STORAGE_KEY_PRESET_GALLERY_CACHE,
  STORAGE_KEY_PRESET_GALLERY_INDEX_URL,
  STORAGE_KEY_PRESET_GALLERY_INSTALLED,
  STORAGE_KEY_THEME_COLOR,
} from "@/lib/appearance-script"

const INDEX_URL = DEFAULT_PRESET_GALLERY_INDEX_URL

function sha256(text: string): string {
  return createHash("sha256").update(text, "utf8").digest("hex")
}

function doc(text: string): GalleryDocument {
  return { text, sha256: sha256(text), bytes: Buffer.byteLength(text) }
}

const sharedLook: AppearancePreset = {
  schemaVersion: 1,
  id: "shared-look",
  name: "Shared look",
  description: "Came in from the gallery.",
  author: "someone",
  base: "rose",
  mode: "dark",
  colors: { light: { primary: "#336699" }, dark: { primary: "#99ccff" } },
  density: { spacing: 0.92 },
}
const sharedText = serializeAppearancePreset(sharedLook)
const sharedUrl = new URL("presets/shared-look.codeg-preset.json", INDEX_URL)
  .href

const quietInk: AppearancePreset = {
  schemaVersion: 1,
  id: "quiet-ink",
  name: "Quiet ink",
  author: "someone else",
  base: "zinc",
  colors: { light: { primary: "#222222" } },
}
const quietText = serializeAppearancePreset(quietInk)
const quietUrl = new URL("presets/quiet-ink.codeg-preset.json", INDEX_URL).href

function listing(
  preset: AppearancePreset,
  text: string,
  overrides: Record<string, unknown> = {}
) {
  return {
    id: preset.id,
    name: preset.name,
    description: preset.description,
    author: preset.author,
    base: preset.base,
    mode: preset.mode,
    url: `presets/${preset.id}.codeg-preset.json`,
    sha256: sha256(text),
    swatches: { light: ["#ffffff", "#336699"], dark: ["#000000", "#99ccff"] },
    ...overrides,
  }
}

const twoListings = JSON.stringify({
  schemaVersion: 1,
  presets: [
    listing(sharedLook, sharedText),
    listing(quietInk, quietText, { swatches: undefined }),
  ],
})

/**
 * A fake backend with the real backend's contract: it hands back the index,
 * and a preset file only when its bytes hash to the digest it was asked for.
 */
function serveGallery(
  index: string,
  files: Record<string, string> = {
    [sharedUrl]: sharedText,
    [quietUrl]: quietText,
  }
) {
  callMock.mockImplementation(
    (command: string, args: { url: string; sha256?: string }) => {
      if (command === "preset_gallery_fetch_index") {
        return Promise.resolve(doc(index))
      }
      if (command === "preset_gallery_fetch_preset") {
        const text = files[args.url]
        if (text === undefined) {
          return Promise.reject(new Error("Preset file returned HTTP 404"))
        }
        if (sha256(text) !== args.sha256) {
          return Promise.reject({
            code: "configuration_invalid",
            message: "Preset file does not match its listed digest.",
            i18n_key: "presetGallery.errors.hashMismatch",
          })
        }
        return Promise.resolve(doc(text))
      }
      return Promise.reject(new Error(`unexpected command ${command}`))
    }
  )
}

function renderSection() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <AppearanceProvider>
        <AppearancePresetGallerySection />
      </AppearanceProvider>
    </NextIntlClientProvider>
  )
}

function rootVar(name: string) {
  return document.documentElement.style.getPropertyValue(name)
}

function installedList(): unknown[] {
  return JSON.parse(
    localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INSTALLED) ?? "[]"
  )
}

function seedInstalled(preset: AppearancePreset, text: string, url: string) {
  localStorage.setItem(
    STORAGE_KEY_PRESET_GALLERY_INSTALLED,
    JSON.stringify([
      {
        preset,
        url,
        indexUrl: INDEX_URL,
        sha256: sha256(text),
        installedAt: "2026-09-01T00:00:00.000Z",
      },
    ])
  )
}

function seedCache(text: string, fetchedAt = new Date().toISOString()) {
  localStorage.setItem(
    STORAGE_KEY_PRESET_GALLERY_CACHE,
    JSON.stringify({ indexUrl: INDEX_URL, fetchedAt, text })
  )
}

beforeEach(() => {
  localStorage.clear()
  document.documentElement.removeAttribute("style")
  document.documentElement.removeAttribute("data-theme")
  document.documentElement.classList.remove("dark")
  callMock.mockReset()
  toastMock.success.mockReset()
  toastMock.error.mockReset()
  themeMock.theme = "light"
  themeMock.setTheme.mockReset()
})

afterEach(() => {
  document.documentElement.removeAttribute("style")
})

describe("AppearancePresetGallerySection", () => {
  it("loads the index when there is no saved copy and draws each card from its listing", async () => {
    serveGallery(twoListings)
    renderSection()

    const card = await screen.findByRole("group", { name: "Shared look" })
    expect(callMock).toHaveBeenCalledWith("preset_gallery_fetch_index", {
      url: INDEX_URL,
    })
    // The strip is the listing's own colours, no image involved.
    const strip = Array.from(card.querySelectorAll("span[style]")).map(
      (el) => (el as HTMLElement).style.backgroundColor
    )
    expect(strip).toEqual(["rgb(255, 255, 255)", "rgb(51, 102, 153)"])
    expect(within(card).getByText("by someone · Dark")).toBeInTheDocument()
    expect(
      within(card).getByText("Came in from the gallery.")
    ).toBeInTheDocument()
    expect(within(card).getByRole("button", { name: "Install" })).toBeEnabled()
    expect(within(card).getByRole("button", { name: "Preview" })).toBeEnabled()
    // A listing without swatches still gets a strip, from its base palette.
    const quiet = screen.getByRole("group", { name: "Quiet ink" })
    expect(quiet.querySelectorAll("span[style]")).toHaveLength(4)
    expect(screen.getByText(/^Last refreshed /)).toBeInTheDocument()
    // The index became the saved copy.
    expect(
      JSON.parse(localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_CACHE)!)
    ).toMatchObject({ indexUrl: INDEX_URL, text: twoListings })
  })

  it("narrows the cards by search", async () => {
    serveGallery(twoListings)
    renderSection()
    await screen.findByRole("group", { name: "Shared look" })

    fireEvent.change(screen.getByRole("textbox", { name: "Search presets…" }), {
      target: { value: "QUIET" },
    })
    expect(screen.queryByRole("group", { name: "Shared look" })).toBeNull()
    expect(screen.getByRole("group", { name: "Quiet ink" })).toBeInTheDocument()

    fireEvent.change(screen.getByRole("textbox", { name: "Search presets…" }), {
      target: { value: "nothing like this" },
    })
    expect(screen.getByText("No presets matched.")).toBeInTheDocument()
  })

  it("installs a verified file: stored with its source and digest, then applied", async () => {
    serveGallery(twoListings)
    renderSection()
    const card = await screen.findByRole("group", { name: "Shared look" })

    fireEvent.click(within(card).getByRole("button", { name: "Install" }))
    await waitFor(() =>
      expect(toastMock.success).toHaveBeenCalledWith("Installed: Shared look")
    )

    // The backend was asked for exactly the listed digest.
    expect(callMock).toHaveBeenCalledWith("preset_gallery_fetch_preset", {
      url: sharedUrl,
      sha256: sha256(sharedText),
    })
    const stored = installedList()
    expect(stored).toHaveLength(1)
    expect(stored[0]).toMatchObject({
      url: sharedUrl,
      indexUrl: INDEX_URL,
      sha256: sha256(sharedText),
      preset: sharedLook,
    })
    // Applied, mode included. The token write is an effect, so wait for it.
    await waitFor(() => expect(rootVar("--primary")).toBe("#336699"))
    expect(document.documentElement.getAttribute("data-theme")).toBe("rose")
    expect(rootVar("--spacing")).toBe("0.23rem")
    expect(themeMock.setTheme).toHaveBeenCalledWith("dark")
    expect(
      JSON.parse(localStorage.getItem(STORAGE_KEY_APPEARANCE_PRESET)!).id
    ).toBe("shared-look")
    // The card now says so.
    expect(within(card).getByText("Installed")).toBeInTheDocument()
    expect(within(card).queryByRole("button", { name: "Install" })).toBeNull()
    expect(within(card).getByRole("button", { name: "Apply" })).toBeEnabled()
    expect(within(card).getByRole("button", { name: "Remove" })).toBeEnabled()
  })

  it("flags an installed preset whose listing changed, and update replaces the copy", async () => {
    const oldLook: AppearancePreset = {
      ...sharedLook,
      colors: { light: { primary: "#000000" }, dark: {} },
    }
    const oldText = serializeAppearancePreset(oldLook)
    seedInstalled(oldLook, oldText, sharedUrl)
    // It is also the current look, so the update must reach the screen.
    localStorage.setItem(STORAGE_KEY_APPEARANCE_PRESET, oldText)
    localStorage.setItem(
      STORAGE_KEY_CUSTOM_THEME,
      JSON.stringify({ light: { primary: "#000000" }, dark: {} })
    )
    document.documentElement.setAttribute("data-theme", "rose")
    serveGallery(twoListings)
    renderSection()

    const card = await screen.findByRole("group", { name: "Shared look" })
    expect(within(card).getByText("Update available")).toBeInTheDocument()
    expect(rootVar("--primary")).toBe("#000000")

    fireEvent.click(within(card).getByRole("button", { name: "Update" }))
    await waitFor(() =>
      expect(toastMock.success).toHaveBeenCalledWith("Updated: Shared look")
    )
    const stored = installedList()
    expect(stored).toHaveLength(1)
    expect(stored[0]).toMatchObject({
      sha256: sha256(sharedText),
      preset: sharedLook,
    })
    await waitFor(() => expect(rootVar("--primary")).toBe("#336699"))
    expect(within(card).queryByText("Update available")).toBeNull()
    expect(within(card).getByText("Installed")).toBeInTheDocument()
  })

  it("does not re-apply an update to a preset that is not the current look", async () => {
    const oldLook: AppearancePreset = {
      ...sharedLook,
      colors: { light: { primary: "#000000" }, dark: {} },
    }
    seedInstalled(oldLook, serializeAppearancePreset(oldLook), sharedUrl)
    document.documentElement.setAttribute("data-theme", "green")
    localStorage.setItem(
      STORAGE_KEY_CUSTOM_THEME,
      JSON.stringify({ light: { primary: "#123456" }, dark: {} })
    )
    serveGallery(twoListings)
    renderSection()
    const card = await screen.findByRole("group", { name: "Shared look" })

    fireEvent.click(within(card).getByRole("button", { name: "Update" }))
    await waitFor(() =>
      expect(toastMock.success).toHaveBeenCalledWith("Updated: Shared look")
    )
    expect(installedList()[0]).toMatchObject({ sha256: sha256(sharedText) })
    expect(document.documentElement.getAttribute("data-theme")).toBe("green")
    expect(rootVar("--primary")).toBe("#123456")
  })

  it("removes only the local copy and leaves the look alone", async () => {
    seedInstalled(sharedLook, sharedText, sharedUrl)
    document.documentElement.setAttribute("data-theme", "rose")
    localStorage.setItem(
      STORAGE_KEY_CUSTOM_THEME,
      JSON.stringify({ light: { primary: "#336699" }, dark: {} })
    )
    serveGallery(twoListings)
    renderSection()
    const card = await screen.findByRole("group", { name: "Shared look" })
    expect(within(card).getByText("Installed")).toBeInTheDocument()

    fireEvent.click(within(card).getByRole("button", { name: "Remove" }))
    expect(toastMock.success).toHaveBeenCalledWith("Removed: Shared look")
    expect(installedList()).toEqual([])
    expect(document.documentElement.getAttribute("data-theme")).toBe("rose")
    expect(rootVar("--primary")).toBe("#336699")
    expect(within(card).getByRole("button", { name: "Install" })).toBeEnabled()
    expect(within(card).queryByText("Installed")).toBeNull()
  })

  it("previews a listed preset and revert restores the previous look exactly", async () => {
    document.documentElement.setAttribute("data-theme", "green")
    localStorage.setItem(STORAGE_KEY_THEME_COLOR, "green")
    const before = { light: { primary: "#111111", radius: "1rem" }, dark: {} }
    localStorage.setItem(STORAGE_KEY_CUSTOM_THEME, JSON.stringify(before))
    serveGallery(twoListings)
    renderSection()
    await waitFor(() => expect(rootVar("--primary")).toBe("#111111"))
    const card = await screen.findByRole("group", { name: "Shared look" })
    const quiet = screen.getByRole("group", { name: "Quiet ink" })

    fireEvent.click(within(card).getByRole("button", { name: "Preview" }))
    await screen.findByText("Previewing Shared look")

    // The preview is the real thing, applied like an install would be.
    expect(document.documentElement.getAttribute("data-theme")).toBe("rose")
    expect(rootVar("--primary")).toBe("#336699")
    expect(rootVar("--spacing")).toBe("0.23rem")
    expect(rootVar("--radius")).toBe("")
    expect(themeMock.setTheme).toHaveBeenCalledWith("dark")
    expect(
      JSON.parse(localStorage.getItem(STORAGE_KEY_APPEARANCE_PRESET)!).id
    ).toBe("shared-look")
    // But it is not an install, and nothing else can be started meanwhile.
    expect(
      localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INSTALLED)
    ).toBeNull()
    expect(
      within(quiet).getByRole("button", { name: "Install" })
    ).toBeDisabled()

    fireEvent.click(screen.getByRole("button", { name: "Revert" }))

    expect(screen.queryByText("Previewing Shared look")).toBeNull()
    expect(document.documentElement.getAttribute("data-theme")).toBe("green")
    expect(rootVar("--primary")).toBe("#111111")
    expect(rootVar("--radius")).toBe("1rem")
    expect(rootVar("--spacing")).toBe("")
    expect(themeMock.setTheme).toHaveBeenLastCalledWith("light")
    expect(localStorage.getItem(STORAGE_KEY_THEME_COLOR)).toBe("green")
    expect(localStorage.getItem(STORAGE_KEY_APPEARANCE_PRESET)).toBeNull()
    expect(
      localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INSTALLED)
    ).toBeNull()
    await waitFor(() =>
      expect(
        JSON.parse(localStorage.getItem(STORAGE_KEY_CUSTOM_THEME)!)
      ).toEqual(before)
    )
    expect(within(quiet).getByRole("button", { name: "Install" })).toBeEnabled()
  })

  it("keeps a preview by installing it, with the look left in place", async () => {
    serveGallery(twoListings)
    renderSection()
    const card = await screen.findByRole("group", { name: "Shared look" })

    fireEvent.click(within(card).getByRole("button", { name: "Preview" }))
    await screen.findByText("Previewing Shared look")
    fireEvent.click(screen.getByRole("button", { name: "Keep and install" }))

    expect(screen.queryByText("Previewing Shared look")).toBeNull()
    expect(toastMock.success).toHaveBeenCalledWith("Installed: Shared look")
    expect(installedList()[0]).toMatchObject({
      sha256: sha256(sharedText),
      preset: sharedLook,
    })
    expect(document.documentElement.getAttribute("data-theme")).toBe("rose")
    expect(rootVar("--primary")).toBe("#336699")
    expect(within(card).getByText("Installed")).toBeInTheDocument()
  })

  it("opens from a fresh saved copy without fetching, and keeps it when a refresh fails", async () => {
    seedCache(twoListings)
    callMock.mockRejectedValue(
      new Error("Preset gallery index fetch failed: offline")
    )
    renderSection()

    expect(
      screen.getByRole("group", { name: "Shared look" })
    ).toBeInTheDocument()
    expect(callMock).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole("button", { name: "Refresh" }))
    const alert = await screen.findByRole("alert")
    expect(
      within(alert).getByText("Could not load the gallery.")
    ).toBeInTheDocument()
    expect(within(alert).getByText(/offline/)).toBeInTheDocument()
    expect(
      within(alert).getByText("Showing the saved copy.")
    ).toBeInTheDocument()
    expect(
      screen.getByRole("group", { name: "Shared look" })
    ).toBeInTheDocument()
    // The failed refresh did not touch the saved copy.
    expect(
      JSON.parse(localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_CACHE)!).text
    ).toBe(twoListings)
  })

  it("refreshes a saved copy that is a day old when it opens", async () => {
    seedCache(twoListings, "2026-09-01T00:00:00.000Z")
    serveGallery(twoListings)
    renderSection()
    expect(
      screen.getByRole("group", { name: "Shared look" })
    ).toBeInTheDocument()
    await waitFor(() =>
      expect(callMock).toHaveBeenCalledWith("preset_gallery_fetch_index", {
        url: INDEX_URL,
      })
    )
  })

  it("refuses an index over the size cap and says so", async () => {
    callMock.mockResolvedValue(doc("x".repeat(MAX_GALLERY_INDEX_BYTES + 1)))
    renderSection()
    const alert = await screen.findByRole("alert")
    expect(
      within(alert).getByText("This gallery index cannot be used.")
    ).toBeInTheDocument()
    expect(
      within(alert).getByText("The file is too large (limit 256 KB)")
    ).toBeInTheDocument()
    expect(localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_CACHE)).toBeNull()
  })

  it("refuses a file the backend reports as not matching its listing, changing nothing", async () => {
    // The listing promises one digest; the file on the server hashes to another.
    serveGallery(twoListings, { [sharedUrl]: `${sharedText}\n` })
    renderSection()
    const card = await screen.findByRole("group", { name: "Shared look" })

    fireEvent.click(within(card).getByRole("button", { name: "Install" }))
    const dialog = await screen.findByRole("dialog")
    expect(
      within(dialog).getByText("This preset cannot be installed")
    ).toBeInTheDocument()
    expect(
      within(dialog).getByText(
        "The file does not match the fingerprint in its listing"
      )
    ).toBeInTheDocument()
    expect(
      localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INSTALLED)
    ).toBeNull()
    expect(document.documentElement.getAttribute("data-theme")).toBeNull()
    expect(rootVar("--primary")).toBe("")
    expect(toastMock.success).not.toHaveBeenCalled()
  })

  it("refuses bytes returned under a digest other than the listed one", async () => {
    // A backend that returned the file but reported a different digest is
    // caught by the second comparison here.
    callMock.mockImplementation((command: string) =>
      Promise.resolve(
        command === "preset_gallery_fetch_index"
          ? doc(twoListings)
          : { ...doc(sharedText), sha256: sha256("something else") }
      )
    )
    renderSection()
    const card = await screen.findByRole("group", { name: "Shared look" })

    fireEvent.click(within(card).getByRole("button", { name: "Install" }))
    const dialog = await screen.findByRole("dialog")
    expect(
      within(dialog).getByText(
        "The file does not match the fingerprint in its listing"
      )
    ).toBeInTheDocument()
    expect(
      localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INSTALLED)
    ).toBeNull()
    expect(rootVar("--primary")).toBe("")
  })

  it("refuses a file that is not a valid preset, naming the field", async () => {
    const withScript = JSON.stringify({ ...sharedLook, script: "alert(1)" })
    const index = JSON.stringify({
      schemaVersion: 1,
      presets: [listing(sharedLook, withScript)],
    })
    serveGallery(index, { [sharedUrl]: withScript })
    renderSection()
    const card = await screen.findByRole("group", { name: "Shared look" })

    fireEvent.click(within(card).getByRole("button", { name: "Install" }))
    const dialog = await screen.findByRole("dialog")
    expect(
      within(dialog).getByText("The file is not a valid preset")
    ).toBeInTheDocument()
    expect(
      within(dialog)
        .getAllByRole("listitem")
        .map((li) => li.textContent)
    ).toEqual(["script: Unknown field"])
    expect(
      localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INSTALLED)
    ).toBeNull()
    expect(rootVar("--primary")).toBe("")
  })

  it("accepts only an https public source, and loads the new one", async () => {
    serveGallery(twoListings)
    renderSection()
    await screen.findByRole("group", { name: "Shared look" })
    callMock.mockClear()

    fireEvent.click(screen.getByRole("button", { name: "Source" }))
    const input = screen.getByLabelText("Default source")
    fireEvent.change(input, {
      target: { value: "http://example.com/index.json" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Use this source" }))
    expect(
      screen.getByText("Enter an https link to a public site.")
    ).toBeInTheDocument()
    expect(callMock).not.toHaveBeenCalled()
    expect(
      localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INDEX_URL)
    ).toBeNull()

    fireEvent.change(input, {
      target: { value: "https://example.com/gallery/index.json" },
    })
    fireEvent.click(screen.getByRole("button", { name: "Use this source" }))
    await waitFor(() =>
      expect(callMock).toHaveBeenCalledWith("preset_gallery_fetch_index", {
        url: "https://example.com/gallery/index.json",
      })
    )
    expect(localStorage.getItem(STORAGE_KEY_PRESET_GALLERY_INDEX_URL)).toBe(
      "https://example.com/gallery/index.json"
    )
  })
})
