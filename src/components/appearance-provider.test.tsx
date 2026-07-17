import { act, fireEvent, render, screen } from "@testing-library/react"
import { beforeEach, describe, expect, it } from "vitest"
import { useMarkdownPreviewPreferences } from "@/hooks/use-appearance"
import { STORAGE_KEY_MARKDOWN_PREVIEW_PRESERVE_LINE_BREAKS } from "@/lib/appearance-script"
import { AppearanceProvider } from "./appearance-provider"

function PreferenceProbe() {
  const {
    markdownPreviewPreserveLineBreaks,
    setMarkdownPreviewPreserveLineBreaks,
  } = useMarkdownPreviewPreferences()

  return (
    <button
      type="button"
      onClick={() => setMarkdownPreviewPreserveLineBreaks(true)}
    >
      {String(markdownPreviewPreserveLineBreaks)}
    </button>
  )
}

describe("AppearanceProvider markdown preview preferences", () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it("defaults to disabled and persists an enabled preference", () => {
    const { unmount } = render(
      <AppearanceProvider>
        <PreferenceProbe />
      </AppearanceProvider>
    )

    expect(screen.getByRole("button")).toHaveTextContent("false")
    fireEvent.click(screen.getByRole("button"))
    expect(screen.getByRole("button")).toHaveTextContent("true")
    expect(
      localStorage.getItem(STORAGE_KEY_MARKDOWN_PREVIEW_PRESERVE_LINE_BREAKS)
    ).toBe("1")

    unmount()
    render(
      <AppearanceProvider>
        <PreferenceProbe />
      </AppearanceProvider>
    )
    expect(screen.getByRole("button")).toHaveTextContent("true")
  })

  it("follows preference changes from another window", () => {
    render(
      <AppearanceProvider>
        <PreferenceProbe />
      </AppearanceProvider>
    )

    localStorage.setItem(STORAGE_KEY_MARKDOWN_PREVIEW_PRESERVE_LINE_BREAKS, "1")
    act(() => {
      window.dispatchEvent(
        new StorageEvent("storage", {
          key: STORAGE_KEY_MARKDOWN_PREVIEW_PRESERVE_LINE_BREAKS,
          newValue: "1",
        })
      )
    })

    expect(screen.getByRole("button")).toHaveTextContent("true")
  })
})
