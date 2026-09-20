import { fireEvent, render, screen } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { describe, expect, it, vi } from "vitest"
import { MemoryKindDialog } from "./memory-kind-dialog"
import enMessages from "@/i18n/messages/en.json"
import type { MemoryKind } from "@/lib/types"

function renderWithIntl(ui: React.ReactElement) {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      {ui}
    </NextIntlClientProvider>
  )
}

describe("MemoryKindDialog", () => {
  it("renders add kind dialog with initial empty inputs", () => {
    renderWithIntl(
      <MemoryKindDialog open onOpenChange={vi.fn()} onSave={vi.fn()} />
    )

    expect(
      screen.getByRole("heading", { name: "Add your own kind" })
    ).toBeInTheDocument()
    expect(screen.getByLabelText("Name")).toHaveValue("")
    expect(screen.getByLabelText("When and what to write")).toHaveValue("")
    expect(screen.getByRole("button", { name: "Save" })).toBeDisabled()
  })

  it("submits the draft when form is filled", () => {
    const handleSave = vi.fn()
    renderWithIntl(
      <MemoryKindDialog
        open={true}
        onOpenChange={vi.fn()}
        onSave={handleSave}
      />
    )

    const nameInput = screen.getByLabelText("Name")
    const instructionInput = screen.getByLabelText("When and what to write")
    const onRequestOption = screen.getByLabelText(/On request/i)

    fireEvent.change(nameInput, { target: { value: "API Quirks" } })
    fireEvent.change(instructionInput, {
      target: { value: "Record external API oddities and workarounds" },
    })
    fireEvent.click(onRequestOption)

    const saveButton = screen.getByRole("button", { name: "Save" })
    expect(saveButton).toBeEnabled()
    fireEvent.click(saveButton)

    expect(handleSave).toHaveBeenCalledWith({
      name: "API Quirks",
      instruction: "Record external API oddities and workarounds",
      mode: "on_request",
    })
  })

  it("populates existing kind fields in edit mode", () => {
    const existingKind: MemoryKind = {
      id: 42,
      key: "custom_notes",
      name: "Architecture Notes",
      instruction: "Record architecture notes",
      mode: "on_request",
      builtin: false,
      enabled: true,
      created_at: "2024-01-10T12:00:00Z",
      updated_at: "2024-01-10T12:00:00Z",
    }

    const handleSave = vi.fn()
    renderWithIntl(
      <MemoryKindDialog
        open={true}
        editingKind={existingKind}
        onOpenChange={vi.fn()}
        onSave={handleSave}
      />
    )

    expect(
      screen.getByRole("heading", { name: "Edit kind" })
    ).toBeInTheDocument()
    expect(screen.getByLabelText("Name")).toHaveValue("Architecture Notes")
    expect(screen.getByLabelText("When and what to write")).toHaveValue(
      "Record architecture notes"
    )

    const nameInput = screen.getByLabelText("Name")
    fireEvent.change(nameInput, { target: { value: "Updated Notes" } })

    fireEvent.click(screen.getByRole("button", { name: "Save" }))
    expect(handleSave).toHaveBeenCalledWith({
      name: "Updated Notes",
      instruction: "Record architecture notes",
      mode: "on_request",
    })
  })

  it("calls onOpenChange(false) when cancel is clicked", () => {
    const handleOpenChange = vi.fn()
    renderWithIntl(
      <MemoryKindDialog
        open={true}
        onOpenChange={handleOpenChange}
        onSave={vi.fn()}
      />
    )

    fireEvent.click(screen.getByRole("button", { name: "Cancel" }))
    expect(handleOpenChange).toHaveBeenCalledWith(false)
  })
})
