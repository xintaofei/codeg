import { render, screen, fireEvent } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import { TemplateGallery } from "./template-gallery"
import { AUTOMATION_TEMPLATES } from "./automation-templates"

const MSGS = enMessages.Automations as Record<string, string>

function renderGallery() {
  const onPick = vi.fn()
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <TemplateGallery onPick={onPick} />
    </NextIntlClientProvider>
  )
  return onPick
}

describe("TemplateGallery", () => {
  it("renders a blank card plus every template", () => {
    renderGallery()
    expect(screen.getByText("Blank automation")).toBeInTheDocument()
    for (const tpl of AUTOMATION_TEMPLATES) {
      expect(screen.getByText(MSGS[tpl.titleKey])).toBeInTheDocument()
    }
    expect(screen.getAllByRole("button")).toHaveLength(
      AUTOMATION_TEMPLATES.length + 1
    )
  })

  it("calls onPick(null) for the blank card", () => {
    const onPick = renderGallery()
    fireEvent.click(screen.getByText("Blank automation"))
    expect(onPick).toHaveBeenCalledWith(null)
  })

  it("calls onPick(template) for a template card", () => {
    const onPick = renderGallery()
    const first = AUTOMATION_TEMPLATES[0]
    fireEvent.click(screen.getByText(MSGS[first.titleKey]))
    expect(onPick).toHaveBeenCalledWith(first)
  })

  it("shows a humanized cadence for scheduled templates and Manual for manual ones", () => {
    renderGallery()
    // code-review is the only weekdays-at template. Match the prefix to avoid
    // the locale time format (ICU uses a narrow no-break space before AM/PM).
    expect(screen.getByText(/Weekdays at/)).toBeInTheDocument()
    expect(screen.getAllByText("Manual").length).toBeGreaterThan(0)
  })
})
