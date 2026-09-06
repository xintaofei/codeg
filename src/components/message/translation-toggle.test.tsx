import { fireEvent, render, screen } from "@testing-library/react"
import { describe, expect, it, vi } from "vitest"

vi.mock("next-intl", () => ({
  useTranslations: () => (key: string) =>
    ({ showOriginal: "Original", showTranslation: "Translation" })[key] ?? key,
}))

import { TranslationToggle } from "./translation-toggle"

describe("TranslationToggle", () => {
  it("offers the view that is not currently displayed", () => {
    const onOriginal = vi.fn()
    const onTranslation = vi.fn()
    const { rerender } = render(
      <TranslationToggle
        isTranslated
        onShowOriginal={onOriginal}
        onShowTranslation={onTranslation}
      />
    )

    fireEvent.click(screen.getByRole("button", { name: "Original" }))
    expect(onOriginal).toHaveBeenCalledTimes(1)

    rerender(
      <TranslationToggle
        isTranslated={false}
        onShowOriginal={onOriginal}
        onShowTranslation={onTranslation}
      />
    )
    fireEvent.click(screen.getByRole("button", { name: "Translation" }))
    expect(onTranslation).toHaveBeenCalledTimes(1)
  })
})
