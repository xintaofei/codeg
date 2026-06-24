import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, describe, expect, it, vi } from "vitest"

vi.mock("@/lib/api", () => ({
  importCcSwitchModelProviders: vi.fn(),
  listImportableCcSwitchModelProviders: vi.fn(),
}))

import { CcSwitchModelProviderImporter } from "./cc-switch-model-provider-importer"
import enMessages from "@/i18n/messages/en.json"
import {
  importCcSwitchModelProviders,
  listImportableCcSwitchModelProviders,
} from "@/lib/api"

function renderImporter() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <CcSwitchModelProviderImporter
        open
        onClose={vi.fn()}
        onDone={vi.fn(async () => {})}
      />
    </NextIntlClientProvider>
  )
}

describe("CcSwitchModelProviderImporter", () => {
  afterEach(() => {
    vi.clearAllMocks()
  })

  it("renders unavailable state when the cc-switch database is missing", async () => {
    vi.mocked(listImportableCcSwitchModelProviders).mockResolvedValueOnce({
      available: false,
      sourcePath: "C:/Users/test/.cc-switch/cc-switch.db",
      items: [],
    })

    renderImporter()

    expect(
      await screen.findByText(/No cc-switch database was found/i)
    ).toBeInTheDocument()
    expect(
      screen.getByText("C:/Users/test/.cc-switch/cc-switch.db")
    ).toBeInTheDocument()
    expect(
      screen.getByRole("button", { name: "Import selected" })
    ).toBeDisabled()
  })

  it("imports the currently selected importable rows and refreshes the preview", async () => {
    const onDone = vi.fn(async () => {})
    vi.mocked(listImportableCcSwitchModelProviders)
      .mockResolvedValueOnce({
        available: true,
        sourcePath: "C:/Users/test/.cc-switch/cc-switch.db",
        items: [
          {
            sourceId: "codex:codex-1",
            sourceAppType: "codex",
            targetAgentType: "codex",
            name: "Codex One",
            apiUrl: "https://api.example.com/v1",
            model: "gpt-5",
            importable: true,
            skipReason: null,
          },
          {
            sourceId: "gemini:gemini-1",
            sourceAppType: "gemini",
            targetAgentType: "gemini",
            name: "Gemini Existing",
            apiUrl: "https://api.gemini.example",
            model: "gemini-3-pro",
            importable: false,
            skipReason: "duplicate_name",
          },
        ],
      })
      .mockResolvedValueOnce({
        available: true,
        sourcePath: "C:/Users/test/.cc-switch/cc-switch.db",
        items: [
          {
            sourceId: "codex:codex-1",
            sourceAppType: "codex",
            targetAgentType: "codex",
            name: "Codex One",
            apiUrl: "https://api.example.com/v1",
            model: "gpt-5",
            importable: false,
            skipReason: "duplicate_name",
          },
        ],
      })
    vi.mocked(importCcSwitchModelProviders).mockResolvedValueOnce({
      importedIds: [101],
      skipped: [],
    })

    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <CcSwitchModelProviderImporter open onClose={vi.fn()} onDone={onDone} />
      </NextIntlClientProvider>
    )

    expect(await screen.findByText("Codex One")).toBeInTheDocument()
    expect(screen.getByText("Already exists by name")).toBeInTheDocument()

    fireEvent.click(
      screen.getByRole("checkbox", { name: "Overwrite same-name providers" })
    )
    fireEvent.click(screen.getByRole("button", { name: "Import selected" }))

    await waitFor(() => {
      expect(importCcSwitchModelProviders).toHaveBeenCalledWith({
        sourceIds: ["codex:codex-1"],
        overwriteSameName: true,
      })
    })
    await waitFor(() => {
      expect(onDone).toHaveBeenCalledTimes(1)
    })
    expect(
      await screen.findByText("Imported 1 provider(s), skipped 0.")
    ).toBeInTheDocument()
    await waitFor(() => {
      expect(
        listImportableCcSwitchModelProviders.mock.calls.length
      ).toBeGreaterThanOrEqual(2)
    })
  })
})
