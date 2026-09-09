import {
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import type {
  TranslationMetricsSnapshot,
  TranslationProviderMetrics,
  TranslationSettings as TranslationSettingsValue,
} from "@/lib/types"

const api = vi.hoisted(() => ({
  getTranslationSettings: vi.fn(),
  updateTranslationSettings: vi.fn(),
  testTranslationSettings: vi.fn(),
  listTranslationModels: vi.fn(),
  getTranslationCacheStats: vi.fn(),
  clearTranslationCache: vi.fn(),
  getTranslationMetrics: vi.fn(),
  getTranslationPoolStatus: vi.fn(),
  resetTranslationProvider: vi.fn(),
  disableTranslationProvider: vi.fn(),
  cooldownTranslationProvider: vi.fn(),
}))
const toast = vi.hoisted(() => ({ success: vi.fn(), error: vi.fn() }))

vi.mock("@/lib/api", () => api)
vi.mock("sonner", () => ({ toast }))
vi.mock("@/hooks/use-translated-text", () => ({
  primeTranslationSettings: vi.fn(),
}))
vi.mock("@/lib/platform", () => ({
  // The pool-status push subscription: tests never fire the event, and the
  // unsubscribe the component registers on mount must be a callable.
  subscribe: vi.fn(() => Promise.resolve(() => {})),
  onTransportReconnect: vi.fn(() => null),
}))

import { TranslationSettings } from "./translation-settings"
import enMessages from "@/i18n/messages/en.json"
import zhCnMessages from "@/i18n/messages/zh-CN.json"

const EMPTY_CACHE = { memoryEntries: 0, diskEntries: 0, diskBytes: 0 }

/** An all-zero snapshot — what a fresh install (or an untouched session) reads. */
function emptyMetrics(): TranslationMetricsSnapshot {
  return {
    dispatchedTotal: 0,
    cacheHits: 0,
    servedTotal: 0,
    gateRejectedTotal: 0,
    gateRejectedInvented: 0,
    gateRejectedEcho: 0,
    gateRejectedDroppedNumbers: 0,
    truncatedTotal: 0,
    providers: {},
    series: {},
  }
}

/** A full per-provider metrics row: zeroed except what the test overrides. */
function providerMetrics(
  overrides: Partial<TranslationProviderMetrics> = {}
): TranslationProviderMetrics {
  return {
    sent: 0,
    ok: 0,
    gateRejected: 0,
    gateRejectedInvented: 0,
    gateRejectedEcho: 0,
    gateRejectedDroppedNumbers: 0,
    rateLimited: 0,
    httpError: 0,
    networkError: 0,
    parseError: 0,
    cacheHits: 0,
    truncated: 0,
    avgLatencyMs: 0,
    dispatchedLastMinute: 0,
    ...overrides,
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
  api.getTranslationCacheStats.mockResolvedValue(EMPTY_CACHE)
  api.clearTranslationCache.mockResolvedValue(EMPTY_CACHE)
  api.getTranslationMetrics.mockResolvedValue(emptyMetrics())
  api.getTranslationPoolStatus.mockResolvedValue([])
  api.resetTranslationProvider.mockResolvedValue(undefined)
  api.disableTranslationProvider.mockResolvedValue([])
  api.cooldownTranslationProvider.mockResolvedValue([])
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
   * dialect (`validate` in `src-tauri/src/translation/settings.rs`); gating the
   * button on a key anyway would put its model list out of reach entirely.
   * Under `auto` the same waiver is read off the host, so `localhost:11434`
   * reaches the list without the user pinning the format first.
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
   * The endpoint card is the loudest thing on the page; with the pool it is
   * per-row, so it stays collapsed behind the list until the user asks for
   * it — the list alone reads clean at a glance.
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
   * cell turns 正常 or 不可用 per its own endpoint, and the toast summarizes.
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
   * The health score used to live only on the badge's hover title; it now has
   * its own column, and a session disabled row carries a Reset button that
   * calls the backend and refreshes the pool strip.
   */
  it("shows the health score and a Reset button for a disabled provider", async () => {
    // Pool status (and metrics) are only fetched once translation is enabled;
    // the strip hides otherwise.
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({ enabled: true })
    )
    api.getTranslationPoolStatus.mockResolvedValue([
      {
        id: "p1",
        name: "Relay",
        baseUrl: "https://relay.example.com",
        model: "m1",
        allowedRpm: 0,
        cooldownRemainingMs: 0,
        disabledReason: "HTTP 401",
        dispatchedLastMinute: 0,
        health: {
          score: 62,
          quality: 0.7,
          stability: 0.5,
          speed: 0.9,
          sample: 20,
          observing: false,
          degraded: true,
        },
      },
    ])
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    // The pool entry itself is never rendered; the row reads off the
    // settings provider, whose (empty) baseUrl falls back to the placeholder.
    await screen.findByText("New provider")

    // The health column carries the rounded score; the full breakdown —
    // degraded note included — rides the hover, and the under-badge line
    // is gone.
    const scoreCell = await screen.findByTitle(
      "Health 62/100 · quality 70% · stability 50% · speed 90% · sample 20 — Below the health threshold: only fallback and probe traffic"
    )
    expect(scoreCell).toHaveTextContent("62")
    expect(scoreCell).toHaveClass("text-amber-600")
    expect(
      screen.queryByText(
        "Health 62/100 · quality 70% · stability 50% · speed 90% · sample 20"
      )
    ).toBeNull()

    const poolReads = api.getTranslationPoolStatus.mock.calls.length
    await userEvent.setup().click(screen.getByRole("button", { name: "Reset" }))
    await waitFor(() =>
      expect(api.resetTranslationProvider).toHaveBeenCalledWith("p1")
    )
    expect(toast.success).toHaveBeenCalledWith("Reset done")
    // The post-reset pool refresh re-reads the status.
    await waitFor(() =>
      expect(api.getTranslationPoolStatus.mock.calls.length).toBeGreaterThan(
        poolReads
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
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({ enabled: true })
    )
    api.getTranslationPoolStatus.mockResolvedValue([
      {
        id: "p1",
        name: "Relay",
        baseUrl: "https://relay.example.com",
        model: "m1",
        allowedRpm: 12,
        cooldownRemainingMs: 0,
        disabledReason: null,
        dispatchedLastMinute: 0,
        health: null,
      },
    ])
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    await screen.findByText("New provider")

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
   * An active provider carries row-level escapes: the session disable calls
   * the backend with just the id and refreshes the pool strip so the row
   * flips to its sidelined state.
   */
  it("disables an active provider from its row", async () => {
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({ enabled: true })
    )
    api.getTranslationPoolStatus.mockResolvedValue([
      {
        id: "p1",
        name: "Relay",
        baseUrl: "https://relay.example.com",
        model: "m1",
        allowedRpm: 12,
        cooldownRemainingMs: 0,
        disabledReason: null,
        dispatchedLastMinute: 0,
        health: null,
      },
    ])
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    await screen.findByText("New provider")

    const poolReads = api.getTranslationPoolStatus.mock.calls.length
    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Disable for this session" }))

    await waitFor(() =>
      expect(api.disableTranslationProvider).toHaveBeenCalledWith("p1")
    )
    expect(toast.success).toHaveBeenCalledWith(
      "Provider disabled for this session"
    )
    await waitFor(() =>
      expect(api.getTranslationPoolStatus.mock.calls.length).toBeGreaterThan(
        poolReads
      )
    )
  })

  /** The cooldown escape shares the disable's row grammar. */
  it("cools an active provider down from its row", async () => {
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({ enabled: true })
    )
    api.getTranslationPoolStatus.mockResolvedValue([
      {
        id: "p1",
        name: "Relay",
        baseUrl: "https://relay.example.com",
        model: "m1",
        allowedRpm: 12,
        cooldownRemainingMs: 0,
        disabledReason: null,
        dispatchedLastMinute: 0,
        health: null,
      },
    ])
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    await screen.findByText("New provider")

    const poolReads = api.getTranslationPoolStatus.mock.calls.length
    await userEvent
      .setup()
      .click(screen.getByRole("button", { name: "Cooldown now" }))

    await waitFor(() =>
      expect(api.cooldownTranslationProvider).toHaveBeenCalledWith("p1")
    )
    expect(toast.success).toHaveBeenCalledWith("Cooldown started")
    await waitFor(() =>
      expect(api.getTranslationPoolStatus.mock.calls.length).toBeGreaterThan(
        poolReads
      )
    )
  })

  /**
   * A sidelined provider (session-disabled or cooling down) has nothing left
   * to disable or cool further: its two row actions collapse into the single
   * Reset that clears the state.
   */
  it("swaps disable and cooldown for a single Restore while sidelined", async () => {
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({ enabled: true })
    )
    api.getTranslationPoolStatus.mockResolvedValue([
      {
        id: "p1",
        name: "Relay",
        baseUrl: "https://relay.example.com",
        model: "m1",
        allowedRpm: 0,
        cooldownRemainingMs: 0,
        disabledReason: "HTTP 401",
        dispatchedLastMinute: 0,
        health: {
          score: 62,
          quality: 0.7,
          stability: 0.5,
          speed: 0.9,
          sample: 20,
          observing: false,
          degraded: true,
        },
      },
    ])
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    await screen.findByText("New provider")

    expect(
      screen.queryByRole("button", { name: "Disable for this session" })
    ).toBeNull()
    expect(screen.queryByRole("button", { name: "Cooldown now" })).toBeNull()
    expect(screen.getByRole("button", { name: "Reset" })).toBeInTheDocument()
  })

  /** An unobserved provider says so instead of printing a made-up score. */
  it("shows the observing note while the health sample is too small", async () => {
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({ enabled: true })
    )
    api.getTranslationPoolStatus.mockResolvedValue([
      {
        id: "p1",
        name: "Relay",
        baseUrl: "https://relay.example.com",
        model: "m1",
        allowedRpm: 12,
        cooldownRemainingMs: 0,
        disabledReason: null,
        dispatchedLastMinute: 0,
        health: {
          score: 70,
          quality: 0.5,
          stability: 0.5,
          speed: 0.5,
          sample: 2,
          observing: true,
          degraded: false,
        },
      },
    ])
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    await screen.findByText("New provider")

    // No score is invented while observing: the cell reads an em dash and
    // the hover explains why.
    expect(
      await screen.findByTitle(
        "Observing — sample too small (2); no dispatch verdict yet"
      )
    ).toHaveTextContent("—")
    expect(screen.queryByRole("button", { name: "Reset" })).toBeNull()
  })

  /**
   * The health score got its own column between State and the row actions:
   * a measured provider shows the rounded score with the breakdown on the
   * hover, and one without a sample reads an em dash instead of an invented
   * number.
   */
  it("shows the health score in its own column or an em dash without data", async () => {
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({
        enabled: true,
        providers: [
          storedProvider({
            id: "p1",
            baseUrl: "https://healthy.example.com",
            model: "m1",
          }),
          storedProvider({
            id: "p2",
            baseUrl: "https://cold.example.com",
            model: "m2",
          }),
        ],
      })
    )
    api.getTranslationPoolStatus.mockResolvedValue([
      {
        id: "p1",
        name: "Healthy",
        baseUrl: "https://healthy.example.com",
        model: "m1",
        allowedRpm: 10,
        cooldownRemainingMs: 0,
        disabledReason: null,
        dispatchedLastMinute: 3,
        health: {
          score: 87.4,
          quality: 0.9,
          stability: 0.8,
          speed: 0.7,
          sample: 40,
          observing: false,
          degraded: false,
        },
      },
      {
        id: "p2",
        name: "Cold",
        baseUrl: "https://cold.example.com",
        model: "m2",
        allowedRpm: 0,
        cooldownRemainingMs: 0,
        disabledReason: null,
        dispatchedLastMinute: 0,
        health: null,
      },
    ])
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    await screen.findByText("https://healthy.example.com")

    expect(screen.getByText("Health")).toBeVisible()

    // The measured provider shows the rounded score; the breakdown lives on
    // the hover only — no visible health line under the badge anymore.
    expect(
      screen.getByTitle(
        "Health 87/100 · quality 90% · stability 80% · speed 70% · sample 40"
      )
    ).toHaveTextContent("87")
    expect(
      screen.queryByText(
        "Health 87/100 · quality 90% · stability 80% · speed 70% · sample 40"
      )
    ).toBeNull()

    // The unmeasured provider reads an em dash in its rate and health cells.
    const coldRow = screen.getByText("https://cold.example.com").closest("tr")
    if (!(coldRow instanceof HTMLElement)) {
      throw new Error("provider row not found")
    }
    expect(within(coldRow).getAllByText("—")).toHaveLength(2)
  })

  /** Metrics need an enabled pool to be fetched at all. */
  function enablePoolWithMetrics(metrics: TranslationMetricsSnapshot) {
    api.getTranslationSettings.mockResolvedValue(
      storedSettings({
        enabled: true,
        providers: [storedProvider({ baseUrl: "https://api.example.com/v1" })],
      })
    )
    api.getTranslationMetrics.mockResolvedValue(metrics)
  }

  /**
   * The call-statistics card is collapsed by default: the header (with its
   * Show/Hide trigger) is visible, the table and trend are not. Expanding
   * reveals the per-provider table; the trigger text flips between
   * Show/Hide across the toggle.
   */
  it("keeps the call statistics collapsed until Show statistics is clicked", async () => {
    const now = Date.now() / 60_000
    enablePoolWithMetrics({
      ...emptyMetrics(),
      providers: {
        p1: providerMetrics({
          sent: 14,
          ok: 12,
          httpError: 1,
          networkError: 1,
          avgLatencyMs: 820,
        }),
      },
      series: {
        p1: [
          {
            minute: Math.floor(now) - 1,
            dispatched: 6,
            ok: 6,
            failed: 0,
            avgLatencyMs: 800,
          },
          {
            minute: Math.floor(now),
            dispatched: 8,
            ok: 6,
            failed: 2,
            avgLatencyMs: 840,
          },
        ],
      },
    })
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    // The pool (and its metrics) only spin up once the saved settings are on.
    await screen.findByText("https://api.example.com/v1")

    // Collapsed: the card header is there, the table and trend are not.
    expect(screen.getByText("Call statistics")).toBeVisible()
    expect(screen.queryByText("Dispatched")).toBeNull()
    expect(screen.queryByText("Avg latency")).toBeNull()

    await user.click(screen.getByRole("button", { name: "Show statistics" }))

    // Expanded: the per-provider table appears with the mock's numbers.
    expect(await screen.findByText("Avg latency")).toBeVisible()
    expect(screen.getByText("api.example.com")).toBeVisible()
    expect(screen.getByText("14")).toBeVisible()
    expect(screen.getByText("12 / 2")).toBeVisible()
    expect(screen.getByText("820 ms")).toBeVisible()

    // The trigger flips its copy while expanded...
    expect(
      screen.getByRole("button", { name: "Hide statistics" })
    ).toBeVisible()

    // ...and collapses again.
    await user.click(screen.getByRole("button", { name: "Hide statistics" }))
    await waitFor(() => expect(screen.queryByText("Avg latency")).toBeNull())
  })

  /**
   * The label grammar: a named provider reads its name, a bare URL reads its
   * host, and a metrics entry whose settings row is gone reads the id prefix.
   */
  it("labels provider rows from name, host, or leftover id", async () => {
    const now = Date.now() / 60_000
    enablePoolWithMetrics({
      ...emptyMetrics(),
      providers: {
        p1: providerMetrics({ sent: 4, ok: 4, avgLatencyMs: 500 }),
        removed0123: providerMetrics({ sent: 2, httpError: 2 }),
      },
      series: {
        p1: [
          {
            minute: Math.floor(now),
            dispatched: 4,
            ok: 4,
            failed: 0,
            avgLatencyMs: 500,
          },
        ],
      },
    })
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    // The pool (and its metrics) only spin up once the saved settings are on.
    await screen.findByText("https://api.example.com/v1")

    await user.click(screen.getByRole("button", { name: "Show statistics" }))

    // The stored row resolves through its host; the removed one through its
    // id prefix, and the zero-latency removed provider shows an em dash.
    expect(await screen.findByText("api.example.com")).toBeVisible()
    expect(screen.getByText("removed0")).toBeVisible()
    expect(screen.getAllByText("—").length).toBeGreaterThan(0)
  })

  /**
   * An empty session (nothing dispatched, no series) has no numbers to show:
   * expanding reveals only the description line, no table, no chart.
   */
  it("shows only the description in the expanded card when there is no data", async () => {
    enablePoolWithMetrics(emptyMetrics())
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    // The pool (and its metrics) only spin up once the saved settings are on.
    await screen.findByText("https://api.example.com/v1")

    await user.click(screen.getByRole("button", { name: "Show statistics" }))

    // The header description stays; the empty body adds a second muted copy
    // of it, and the table/chart must not appear. With no table there is no
    // header summary either — the counters are table-borne.
    expect(
      await screen.findAllByText(
        "Session dispatch volume, outcomes, and latency; cleared on restart"
      )
    ).not.toHaveLength(0)
    expect(screen.queryByText("Avg latency")).toBeNull()
    expect(screen.queryByText("Dispatched")).toBeNull()
    expect(screen.queryByText(/Cache hits/)).toBeNull()
  })

  /**
   * The gate counters are per-provider table columns now: the old session-wide
   * rollup (first a metricsSummary line, then a header-cell blob) is gone, so
   * no global summary renders anywhere and the outcome columns exist only in
   * the expanded table.
   */
  it("shows the outcome columns only inside the expanded statistics table", async () => {
    enablePoolWithMetrics({
      ...emptyMetrics(),
      dispatchedTotal: 5,
      servedTotal: 4,
      cacheHits: 2,
      providers: {
        p1: providerMetrics({
          sent: 5,
          ok: 4,
          httpError: 1,
          avgLatencyMs: 700,
          cacheHits: 2,
        }),
      },
      series: {
        p1: [
          {
            minute: Math.floor(Date.now() / 60_000),
            dispatched: 5,
            ok: 4,
            failed: 1,
            avgLatencyMs: 700,
          },
        ],
      },
    })
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    // The pool (and its metrics) only spin up once the saved settings are on.
    await screen.findByText("https://api.example.com/v1")

    // Collapsed: no outcome column header anywhere on the page.
    expect(screen.queryByText("Cache hits")).toBeNull()
    expect(screen.queryByText("Truncated")).toBeNull()

    await user.click(screen.getByRole("button", { name: "Show statistics" }))

    // Expanded: the per-provider columns are there (the row shows this
    // provider's own hit count), and neither the old global summary blob nor
    // the retired metricsSummary line comes back.
    expect(await screen.findByText("Cache hits")).toBeVisible()
    expect(screen.queryByText(/Cache hits 2 ·/)).toBeNull()
    expect(screen.queryByText(/served 4/)).toBeNull()

    // The provider list card carries no outcome column either.
    const providerCard = screen
      .getByText("Current rate")
      .closest("div.divide-y")
    if (!(providerCard instanceof HTMLElement)) {
      throw new Error("provider card not found")
    }
    expect(within(providerCard).queryByText("Cache hits")).toBeNull()
  })

  /**
   * Every outcome is its own column and every row carries that provider's
   * own counters: two providers with different numbers must not bleed into
   * each other, and a snapshot without the per-provider outcome fields reads
   * em dashes — never fake zeros.
   */
  it("renders the six outcome columns with each provider's own counters", async () => {
    enablePoolWithMetrics({
      ...emptyMetrics(),
      providers: {
        p1: providerMetrics({
          sent: 8,
          ok: 6,
          avgLatencyMs: 900,
          cacheHits: 5,
          gateRejected: 4,
          gateRejectedInvented: 3,
          gateRejectedEcho: 1,
          gateRejectedDroppedNumbers: 0,
          truncated: 2,
        }),
        // Removed from the settings list: the row reads the id prefix.
        retired99: providerMetrics({
          sent: 9,
          ok: 6,
          avgLatencyMs: 45,
          cacheHits: 1,
          gateRejected: 4,
          gateRejectedInvented: 2,
          gateRejectedEcho: 3,
          gateRejectedDroppedNumbers: 0,
          truncated: 5,
        }),
        // A snapshot from before the per-provider outcome columns: only the
        // pre-existing counters are present, the new ones are undefined.
        legacy: {
          sent: 5,
          ok: 1,
          gateRejected: 2,
          rateLimited: 0,
          httpError: 2,
          networkError: 0,
          parseError: 0,
          avgLatencyMs: 40,
          dispatchedLastMinute: 0,
        } as TranslationProviderMetrics,
      },
      series: {
        p1: [
          {
            minute: Math.floor(Date.now() / 60_000),
            dispatched: 8,
            ok: 6,
            failed: 2,
            avgLatencyMs: 900,
          },
        ],
      },
    })
    const user = userEvent.setup()
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <TranslationSettings />
      </NextIntlClientProvider>
    )
    // The pool (and its metrics) only spin up once the saved settings are on.
    await screen.findByText("https://api.example.com/v1")

    await user.click(screen.getByRole("button", { name: "Show statistics" }))

    // The six new column headers are all present.
    for (const header of [
      "Cache hits",
      "Rejected",
      "Invented",
      "Echo/refusal",
      "Dropped",
      "Truncated",
    ]) {
      expect(screen.getByText(header)).toBeVisible()
    }

    // Rows sort by dispatch volume: retired99 (9), p1 (8), legacy (5).
    // p1's row carries its own counters, nothing else's.
    const p1Row = screen.getByText("api.example.com").closest("tr")
    if (!(p1Row instanceof HTMLElement)) {
      throw new Error("p1 row not found")
    }
    expect(within(p1Row).getByText("5")).toBeVisible() // cache hits
    expect(within(p1Row).getByText("4")).toBeVisible() // rejected
    expect(within(p1Row).getByText("3")).toBeVisible() // invented
    expect(within(p1Row).getByText("1")).toBeVisible() // echo/refusal
    expect(within(p1Row).getByText("0")).toBeVisible() // dropped numbers
    expect(within(p1Row).getByText("2")).toBeVisible() // truncated

    // The retired provider's row carries its own, different counters.
    const retiredRow = screen.getByText("retired9").closest("tr")
    if (!(retiredRow instanceof HTMLElement)) {
      throw new Error("retired row not found")
    }
    expect(within(retiredRow).getByText("1")).toBeVisible() // cache hits
    expect(within(retiredRow).getByText("4")).toBeVisible() // rejected
    expect(within(retiredRow).getByText("2")).toBeVisible() // invented
    expect(within(retiredRow).getByText("3")).toBeVisible() // echo/refusal
    expect(within(retiredRow).getByText("0")).toBeVisible() // dropped numbers
    expect(within(retiredRow).getByText("5")).toBeVisible() // truncated

    // The stale snapshot row: the rejected counter is a real number, while
    // the five absent per-provider outcome fields all read em dashes —
    // missing data never masquerades as a zero.
    const legacyRow = screen.getByText("legacy").closest("tr")
    if (!(legacyRow instanceof HTMLElement)) {
      throw new Error("legacy row not found")
    }
    expect(within(legacyRow).getByText("2")).toBeVisible() // rejected
    expect(within(legacyRow).getAllByText("—")).toHaveLength(5)
  })
})
