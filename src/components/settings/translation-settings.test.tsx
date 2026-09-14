import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type {
  TranslationSettings as TranslationSettingsValue,
  TranslationStats,
} from "@/lib/types"

const api = vi.hoisted(() => ({
  getTranslationSettings: vi.fn(),
  updateTranslationSettings: vi.fn(),
  testTranslationSettings: vi.fn(),
  listTranslationModels: vi.fn(),
  getTranslationStats: vi.fn(),
}))
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }))

vi.mock("@/lib/api", () => api)
vi.mock("sonner", () => ({ toast }))
vi.mock("@/hooks/use-translated-text", () => ({
  primeTranslationSettings: vi.fn(),
}))

import { TranslationSettings } from "./translation-settings"
import enMessages from "@/i18n/messages/en.json"
import zhCnMessages from "@/i18n/messages/zh-CN.json"

/** An all-zero snapshot — what a fresh install (or an untouched session) reads. */
function emptyStats(): TranslationStats {
  return {
    requests: 0,
    ok: 0,
    rejected: 0,
    failures: 0,
    avgLatencyMs: 0,
  }
}

function storedProvider(
  overrides: Partial<TranslationSettingsValue["providers"][number]> = {}
): TranslationSettingsValue["providers"][number] {
  return {
    id: "p1",
    name: null,
    baseUrl: "",
    apiKey: "",
    model: "",
    apiFormat: "auto",
    enabled: true,
    rpmCap: null,
    ...overrides,
  }
}

function storedSettings(
  overrides: Partial<TranslationSettingsValue> = {}
): TranslationSettingsValue {
  return {
    enabled: false,
    providers: [storedProvider()],
    baseUrl: "",
    apiKey: "",
    model: "",
    targetLang: null,
    translateBody: true,
    translateThinking: false,
    apiFormat: "auto",
    selectionTranslate: true,
    selectionTargetLang: null,
    toggleAlwaysVisible: false,
    priorityMaxConcurrent: null,
    backgroundMaxConcurrent: null,
    batchMaxChars: null,
    failureThreshold: null,
    cooldownSeconds: null,
    carryContext: true,
    ...overrides,
  }
}

beforeEach(() => {
  vi.clearAllMocks()
  // jsdom has no layout engine; the provider editor scrolls itself into
  // view when it opens.
  Element.prototype.scrollIntoView = vi.fn()
  api.getTranslationSettings.mockResolvedValue(storedSettings())
  api.updateTranslationSettings.mockImplementation(async (settings) => settings)
  api.testTranslationSettings.mockResolvedValue("ok")
  api.listTranslationModels.mockResolvedValue([])
  api.getTranslationStats.mockResolvedValue(emptyStats())
})

/**
 * Renders and waits out the initial read the page gates its rows on, then
 * opens the provider editor — the endpoint card is collapsed behind the list
 * by default, and every field-level test below edits a row.
 */
async function renderPage() {
  render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <TranslationSettings />
    </NextIntlClientProvider>
  )
  await userEvent
    .setup()
    .click(await screen.findByRole("button", { name: "Edit" }))
  await screen.findByLabelText("Base URL")
}

/** The two fields the fetch button is gated on. */
async function fillCredentials(user: ReturnType<typeof userEvent.setup>) {
  await user.type(screen.getByLabelText("Base URL"), "api.example.com")
  await user.type(screen.getByLabelText("API key"), "sk-test")
}

/** The dropdown picker only exists while a matching probe has models to offer. */
function modelPicker(): HTMLButtonElement | null {
  return screen.queryByRole("combobox", {
    name: "Pick a fetched model",
  }) as HTMLButtonElement | null
}

/**
 * Same wiring this page's grammar carries as every other settings tab: a
 * `SettingRow` whose `htmlFor` is missing still *looks* right while silently
 * leaving its control unlabeled for assistive tech (see general-settings.test).
 * These assertions are what catch that.
 */
/** The checkbox a scope dropdown row carries: visual-only (aria-hidden), so
 *  it is reached through its cmdk option rather than by role. */
function scopeCheckbox(option: HTMLElement): HTMLElement {
  const checkbox = option.querySelector('[data-slot="checkbox"]')
  if (!(checkbox instanceof HTMLElement)) {
    throw new Error("scope option has no checkbox")
  }
  return checkbox
}

describe("TranslationSettings", () => {
  it("wires every row's label to the control it names", async () => {
    await renderPage()

    const enabled = await screen.findByLabelText("Enable translation")
    expect(enabled).toHaveAttribute("role", "switch")

    // A bare host is the point of the normalizing backend: the placeholder
    // has to advertise that, not a fully-specified `https://…/v1`.
    expect(screen.getByLabelText("Base URL")).toHaveAttribute(
      "placeholder",
      "api.example.com"
    )
    expect(screen.getByLabelText("API key")).toHaveAttribute("type", "password")
    expect(screen.getByLabelText("API format")).toBeInTheDocument()
    expect(screen.getByLabelText("Model")).toBeInTheDocument()
    // The picker is not mounted until a probe has models to offer.
    expect(modelPicker()).toBeNull()
    expect(screen.getByLabelText("Target language")).toBeInTheDocument()
  })

  /**
   * `Language` is keyed by language name while `APP_LOCALES` carries locale
   * codes, so feeding a code straight to the translator resolves nothing and
   * next-intl renders the key back — the picker then reads "zh_cn" instead of
   * "Simplified Chinese".
   */
  it("names every target language instead of echoing its locale code", async () => {
    await renderPage()

    const picker = await screen.findByLabelText("Target language")
    expect(picker).toHaveTextContent("Follow interface language")

    fireEvent.click(picker)

    for (const language of ["English", "Simplified Chinese", "Arabic"]) {
      expect(
        await screen.findByRole("option", { name: language })
      ).toBeVisible()
    }
    expect(screen.queryByRole("option", { name: "zh_cn" })).toBeNull()
  })

  it("cannot fetch models before an endpoint and a key exist", async () => {
    const user = userEvent.setup()
    await renderPage()

    const fetchModels = screen.getByRole("button", { name: "Fetch models" })
    expect(fetchModels).toBeDisabled()

    await user.type(screen.getByLabelText("Base URL"), "api.example.com")
    expect(fetchModels).toBeDisabled()

    await user.type(screen.getByLabelText("API key"), "sk-test")
    expect(fetchModels).toBeEnabled()
    expect(api.listTranslationModels).not.toHaveBeenCalled()
  })

  /**
   * The list is a real picker, not a suggestion channel: it opens next to the
   * fetch button and choosing a model writes it into the field. Typing a model
   * the endpoint does not advertise still works — the input stays editable.
   */
  it("offers the fetched models in a dropdown that fills the model field", async () => {
    api.listTranslationModels.mockResolvedValue([
      "gpt-4o-mini",
      "claude-sonnet-4-5",
    ])
    const user = userEvent.setup()
    await renderPage()
    await fillCredentials(user)

    await user.click(screen.getByRole("button", { name: "Fetch models" }))

    const picker = await screen.findByRole("combobox", {
      name: "Pick a fetched model",
    })
    await user.click(picker)
    await user.click(await screen.findByRole("option", { name: "gpt-4o-mini" }))
    expect(api.listTranslationModels).toHaveBeenCalledWith(
      expect.objectContaining({
        providers: [
          expect.objectContaining({
            baseUrl: "api.example.com",
            apiKey: "sk-test",
            apiFormat: "auto",
          }),
        ],
      }),
      "p1"
    )
    expect(screen.getByLabelText("Model")).toHaveValue("gpt-4o-mini")
    expect(toast.error).not.toHaveBeenCalled()
  })

  /**
   * The backend distinguishes a bad key from an endpoint with no model route;
   * collapsing that into a generic failure would strand the user on the one
   * screen where the distinction is actionable.
   */
  it("surfaces the backend's own message when the fetch fails", async () => {
    api.listTranslationModels.mockRejectedValue({
      code: "configuration_invalid",
      message: "Could not list models",
      detail:
        "This endpoint does not expose a model list — enter the model name manually",
    })
    const user = userEvent.setup()
    await renderPage()
    await fillCredentials(user)

    await user.click(screen.getByRole("button", { name: "Fetch models" }))

    await waitFor(() => expect(toast.error).toHaveBeenCalledTimes(1))
    expect(toast.error).toHaveBeenCalledWith(
      expect.stringContaining("does not expose a model list")
    )
    expect(modelPicker()).toBeNull()
  })

  /** An endpoint that answers with an empty list is working, not broken. */
  it("explains an empty list inline instead of raising an error", async () => {
    api.listTranslationModels.mockResolvedValue([])
    const user = userEvent.setup()
    await renderPage()
    await fillCredentials(user)

    await user.click(screen.getByRole("button", { name: "Fetch models" }))

    expect(
      await screen.findByText("The endpoint returned no models")
    ).toBeVisible()
    expect(toast.error).not.toHaveBeenCalled()
    expect(modelPicker()).toBeNull()
  })

  /**
   * A list only describes the endpoint it came from. Leaving it up after the
   * URL moves would suggest models for a request that is no longer the one the
   * page would issue.
   */
  it("drops the suggestions once the base URL changes", async () => {
    api.listTranslationModels.mockResolvedValue(["gpt-4o-mini"])
    const user = userEvent.setup()
    await renderPage()
    await fillCredentials(user)
    await user.click(screen.getByRole("button", { name: "Fetch models" }))
    await waitFor(() => expect(modelPicker()).toBeVisible())

    fireEvent.change(screen.getByLabelText("Base URL"), {
      target: { value: "api.example.com/v1" },
    })

    expect(modelPicker()).toBeNull()
  })

  /**
   * Same reasoning for the dialect: one base URL answers `/v1/models` and
   * `/v1beta/openai/models` with different catalogues, so the format is part
   * of what a list speaks for.
   */
  it("drops the suggestions once the API format changes", async () => {
    api.listTranslationModels.mockResolvedValue(["gpt-4o-mini"])
    const user = userEvent.setup()
    await renderPage()
    await fillCredentials(user)
    await user.click(screen.getByRole("button", { name: "Fetch models" }))
    await waitFor(() => expect(modelPicker()).toBeVisible())

    await user.click(screen.getByRole("combobox", { name: "API format" }))
    await user.click(await screen.findByRole("option", { name: "Claude" }))

    await waitFor(() => expect(modelPicker()).toBeNull())
  })

  /**
   * The switches only move local state; the backend (and every renderer) moves
   * when 保存 runs. Without the hint a toggled-but-unsaved page looks applied —
   * the exact trap that reads as "the feature ignores its own switch".
   */
  it("flags unsaved changes until a save lands", async () => {
    const user = userEvent.setup()
    await renderPage()
    expect(screen.queryByText(/Unsaved changes/)).toBeNull()

    await user.click(screen.getByLabelText("Enable translation"))
    expect(screen.getByText(/Unsaved changes/)).toBeVisible()

    await user.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() =>
      expect(screen.queryByText(/Unsaved changes/)).toBeNull()
    )
  })

  /**
   * The backend speaks English constants; the toasts must not. A known
   * validation message maps to the interface's language (asserted in
   * zh-CN, where the translation differs from the source), an unknown one
   * passes through untouched rather than being mistranslated.
   */
  it("localizes known backend validation messages in toasts", async () => {
    api.updateTranslationSettings.mockRejectedValue({
      code: "configuration_missing",
      message:
        "Translation needs at least one enabled provider with a base URL, an API key, and a model",
    })
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="zh-CN" messages={zhCnMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    await user.click(await screen.findByRole("button", { name: "编辑" }))
    await screen.findByLabelText("Base URL")

    await user.click(screen.getByRole("button", { name: "保存" }))

    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "翻译至少需要一个启用的供应商，并填好 Base URL、API 密钥和模型"
      )
    )
  })

  it("saves the format picked from the dropdown", async () => {
    const user = userEvent.setup()
    await renderPage()

    await user.click(screen.getByRole("combobox", { name: "API format" }))
    await user.click(await screen.findByRole("option", { name: "Claude" }))

    // The placeholder is the visible proof the draft moved with the picker.
    expect(screen.getByLabelText("Model")).toHaveAttribute(
      "placeholder",
      "claude-sonnet-4-5"
    )

    await user.click(screen.getByRole("button", { name: "Save" }))

    await waitFor(() =>
      expect(api.updateTranslationSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          providers: [
            expect.objectContaining({ apiFormat: "anthropic", id: "p1" }),
          ],
        })
      )
    )
  })

  /**
   * Ollama serves locally with no auth and the backend waives the key for that
   * dialect; gating the button on a key anyway would put its model list out of
   * reach entirely. Under `auto` the same waiver is read off the host, so
   * `localhost:11434` reaches the list without the user pinning the format
   * first.
   */
  it("lists Ollama models without asking for a key", async () => {
    api.listTranslationModels.mockResolvedValue(["qwen2.5:14b"])
    const user = userEvent.setup()
    await renderPage()

    // Every other dialect still needs one — the waiver is not a blanket one.
    await user.type(screen.getByLabelText("Base URL"), "api.example.com")
    await user.type(screen.getByLabelText("API key"), "sk-test")
    expect(screen.getByRole("button", { name: "Fetch models" })).toBeEnabled()

    await user.clear(screen.getByLabelText("API key"))
    expect(screen.getByRole("button", { name: "Fetch models" })).toBeDisabled()

    await user.clear(screen.getByLabelText("Base URL"))
    await user.type(screen.getByLabelText("Base URL"), "localhost:11434")

    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Fetch models" })).toBeEnabled()
    )
    await user.click(screen.getByRole("button", { name: "Fetch models" }))

    await waitFor(() => expect(modelPicker()).toBeVisible())
    expect(api.listTranslationModels).toHaveBeenCalledWith(
      expect.objectContaining({
        providers: [expect.objectContaining({ apiKey: "", apiFormat: "auto" })],
      }),
      "p1"
    )
  }, 15000)

  /**
   * The endpoint card is the loudest thing on the page; it stays collapsed
   * behind the list until the user asks for it — the list alone reads clean
   * at a glance.
   */
  it("keeps the endpoint editor collapsed until a row is edited", async () => {
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    await screen.findByText("New provider")

    expect(screen.queryByLabelText("Base URL")).toBeNull()

    await user.click(screen.getByRole("button", { name: "Edit" }))
    await waitFor(() =>
      expect(screen.getByLabelText("Base URL")).toBeInTheDocument()
    )

    await user.click(screen.getByRole("button", { name: "Done" }))
    await waitFor(() => expect(screen.queryByLabelText("Base URL")).toBeNull())
  })

  it("opens a fresh editor for a newly added provider", async () => {
    const user = userEvent.setup()
    await renderPage()

    await user.click(screen.getByRole("button", { name: "Add provider" }))

    // The new row's editor is open with empty draft fields...
    expect(screen.getByLabelText("Base URL")).toHaveValue("")
    // ...and the list now shows two rows (the stored one plus the draft).
    expect(screen.getAllByRole("button", { name: "Edit" })).toHaveLength(2)
  })

  it("opens the editor by itself on a fresh install with no providers", async () => {
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({ providers: [] })
    )
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )

    // Nothing to edit in the list means nothing to collapse: the empty draft
    // must be visible immediately or a fresh install shows no way forward.
    expect(await screen.findByLabelText("Base URL")).toBeInTheDocument()
    expect(screen.getByRole("button", { name: "Done" })).toBeInTheDocument()
  })

  it("disables delete on the last remaining provider", async () => {
    await renderPage()
    expect(
      screen.getByRole("button", { name: "Remove provider" })
    ).toBeDisabled()
  })

  /**
   * 测试连接 runs EVERY provider, not just the row in the editor: each state
   * cell turns OK or unavailable per its own endpoint, and the toast
   * summarizes.
   */
  it("tests every provider and shows each verdict in the state column", async () => {
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({
        providers: [
          storedProvider({
            id: "p1",
            baseUrl: "https://good.example.com",
            model: "m1",
          }),
          storedProvider({
            id: "p2",
            baseUrl: "https://bad.example.com",
            model: "m2",
          }),
        ],
      })
    )
    api.testTranslationSettings.mockImplementation(
      async (_settings, _locale, providerId) => {
        if (providerId === "p2") {
          throw {
            code: "network",
            message: "The translation service returned HTTP 401",
          }
        }
        return "Hello, this is a connection test."
      }
    )
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    await screen.findByText("https://good.example.com")

    await user.click(screen.getByRole("button", { name: "Test connection" }))

    // Both rows were tested, each with its own id.
    await waitFor(() =>
      expect(api.testTranslationSettings).toHaveBeenCalledTimes(2)
    )
    expect(api.testTranslationSettings).toHaveBeenCalledWith(
      expect.anything(),
      "en",
      "p1"
    )
    expect(api.testTranslationSettings).toHaveBeenCalledWith(
      expect.anything(),
      "en",
      "p2"
    )
    // One OK, one unavailable, shown in each row's own state cell.
    await waitFor(() =>
      expect(toast.error).toHaveBeenCalledWith(
        "1 of 2 providers OK — see the state column for the rest"
      )
    )
    expect(screen.getByText("unavailable")).toBeInTheDocument()
  })

  /**
   * The three scope switches collapsed into one multi-select: opening it lists
   * every lane as a checkbox row, and unchecking one is a settings edit like
   * any other — the unsaved hint must appear so 保存 is the obvious next step.
   */
  it("toggles body translation through the scope dropdown", async () => {
    const user = userEvent.setup()
    await renderPage()

    expect(screen.getByText("Translation scope")).toBeVisible()

    const trigger = screen.getByRole("combobox", {
      name: "Translation scope",
    })
    // Defaults read as a summary on the closed trigger: body on, thinking off.
    expect(trigger).toHaveTextContent("Translate reply body")

    await user.click(trigger)

    const bodyOption = await screen.findByRole("option", {
      name: "Translate reply body",
    })
    // The checked state shows as the row's checkbox tick (aria-hidden inside
    // the cmdk option); `data-state` on that checkbox is what to assert.
    expect(scopeCheckbox(bodyOption)).toHaveAttribute("data-state", "checked")
    const thinkingOption = screen.getByRole("option", {
      name: "Translate thinking blocks",
    })
    expect(scopeCheckbox(thinkingOption)).toHaveAttribute(
      "data-state",
      "unchecked"
    )
    const selectionOption = screen.getByRole("option", {
      name: "Selection translation",
    })
    expect(scopeCheckbox(selectionOption)).toHaveAttribute(
      "data-state",
      "checked"
    )

    await user.click(bodyOption)
    expect(
      scopeCheckbox(
        screen.getByRole("option", { name: "Translate reply body" })
      )
    ).toHaveAttribute("data-state", "unchecked")
    // Toggling a scope lane is a settings edit like any other: the unsaved
    // hint must appear so 保存 is the obvious next step.
    expect(screen.getByText(/Unsaved changes/)).toBeVisible()

    await user.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() =>
      expect(api.updateTranslationSettings).toHaveBeenCalledWith(
        expect.objectContaining({ translateBody: false })
      )
    )
  })

  /**
   * 划词 keeps a dependent row: the selection target language picker only
   * exists while the selection lane is one of the checked scopes. Unchecking
   * the lane inside the dropdown hides the row immediately.
   */
  it("shows the selection target language row only while the lane is checked", async () => {
    const user = userEvent.setup()
    await renderPage()

    expect(
      screen.getByLabelText("Selection target language")
    ).toBeInTheDocument()

    await user.click(
      screen.getByRole("combobox", { name: "Translation scope" })
    )
    await user.click(
      await screen.findByRole("option", { name: "Selection translation" })
    )
    // The dropdown stays open across toggles; close it to see the rows again.
    await user.keyboard("{Escape}")

    expect(screen.queryByLabelText("Selection target language")).toBeNull()
  })

  /**
   * The two lane-concurrency fields follow the batch-ceiling grammar exactly:
   * empty means "use the default" (`null` payload), typed digits move local
   * state, and a cleared field returns to `null`.
   */
  it("binds the lane concurrency fields with the same null-or-number grammar", async () => {
    const user = userEvent.setup()
    await renderPage()

    const priority = screen.getByLabelText("Priority lane concurrency")
    const background = screen.getByLabelText("Background lane concurrency")
    expect(priority).toHaveValue(null)
    expect(background).toHaveValue(null)
    expect(priority).toHaveAttribute("placeholder", "Default: 4")
    expect(background).toHaveAttribute("placeholder", "Default: 3")

    fireEvent.change(priority, { target: { value: "6" } })
    expect(priority).toHaveValue(6)
    expect(screen.getByText(/Unsaved changes/)).toBeVisible()

    fireEvent.change(background, { target: { value: "2" } })
    expect(background).toHaveValue(2)

    await user.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() =>
      expect(api.updateTranslationSettings).toHaveBeenCalledWith(
        expect.objectContaining({
          priorityMaxConcurrent: 6,
          backgroundMaxConcurrent: 2,
        })
      )
    )
    // The button re-enables only after the save's read-back settles; without
    // this a second click could race the still-disabled control.
    await waitFor(() =>
      expect(screen.getByRole("button", { name: "Save" })).toBeEnabled()
    )

    // The save refreshes from the (mocked) store, so the form reads back the
    // defaults; clearing the field must land as `null`, never as 0 or NaN.
    fireEvent.change(priority, { target: { value: "" } })
    expect(priority).toHaveValue(null)
    await user.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() =>
      expect(api.updateTranslationSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({
          priorityMaxConcurrent: null,
          backgroundMaxConcurrent: null,
        })
      )
    )
  })

  /**
   * The failure strategy is a page-level knob, not a per-row one: it lives
   * behind the table header so the provider list stays the headline. Typed
   * digits reach the saved settings; the placeholders advertise the defaults
   * an empty field keeps.
   */
  it("puts the failure strategy behind the table header and saves it", async () => {
    const user = userEvent.setup()
    await renderPage()

    await user.click(screen.getByRole("button", { name: "Failure strategy" }))

    const threshold = await screen.findByLabelText("Failure threshold")
    expect(threshold).toHaveAttribute("placeholder", "Default: 3")
    const cooldown = await screen.findByLabelText("Cooldown (seconds)")
    expect(cooldown).toHaveAttribute("placeholder", "Default: 60")

    await user.type(threshold, "5")
    await user.type(cooldown, "120")

    await user.click(screen.getByRole("button", { name: "Save" }))
    await waitFor(() =>
      expect(api.updateTranslationSettings).toHaveBeenLastCalledWith(
        expect.objectContaining({
          failureThreshold: 5,
          cooldownSeconds: 120,
        })
      )
    )
  })

  /**
   * The call statistics card is a single line of session counters for the
   * configured endpoint: dispatch volume, gate rejections, outright
   * failures, mean latency. A failed read renders the empty line instead of
   * tearing the page down — the numbers are informational only.
   */
  it("shows the single-line call statistics", async () => {
    api.getTranslationStats.mockResolvedValue({
      requests: 14,
      ok: 10,
      rejected: 2,
      failures: 2,
      avgLatencyMs: 820.4,
    })
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )

    expect(await screen.findByText(/Call statistics/)).toBeVisible()
    expect(
      await screen.findByText(
        "Requests 14 · OK 10 · Rejected 2 · Failed 2 · Avg latency 820 ms"
      )
    ).toBeVisible()
  })

  it("falls back to an empty statistics line when the read fails", async () => {
    api.getTranslationStats.mockRejectedValue(new Error("offline"))
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )

    expect(await screen.findByText("Call statistics")).toBeVisible()
    expect(
      await screen.findByText(
        "No requests in this session yet; counters reset on restart."
      )
    ).toBeVisible()
  })
})
