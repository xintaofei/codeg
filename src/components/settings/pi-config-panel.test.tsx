import { act, fireEvent, render, screen, waitFor } from "@testing-library/react"
import userEvent from "@testing-library/user-event"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import enMessages from "@/i18n/messages/en.json"
import type { AcpAgentInfo } from "@/lib/types"
import { PiConfigPanel, piRuntimeIsTooOld } from "./pi-config-panel"

const api = vi.hoisted(() => ({
  loadPiConfig: vi.fn(),
  listPiModelCapabilities: vi.fn(),
  acpUpdatePiConfig: vi.fn(),
  acpValidatePiCommand: vi.fn(),
  acpPiListTrustEntries: vi.fn(),
  onSaveEnv: vi.fn(),
}))

vi.mock("@/lib/api", () => ({
  loadPiConfig: api.loadPiConfig,
  listPiModelCapabilities: api.listPiModelCapabilities,
  acpUpdatePiConfig: api.acpUpdatePiConfig,
  acpValidatePiCommand: api.acpValidatePiCommand,
  acpPiListTrustEntries: api.acpPiListTrustEntries,
  acpPiSetProjectTrust: vi.fn(),
  acpInstallPiBinary: vi.fn(),
  acpUninstallPiBinary: vi.fn(),
}))

const CAPABILITIES = [
  {
    provider: "openai",
    id: "gpt-5.6-sol",
    reasoning: true,
    thinkingLevelMap: { minimal: null, xhigh: "xhigh", max: "max" },
  },
  {
    provider: "openai",
    id: "gpt-4",
    reasoning: false,
    thinkingLevelMap: {},
  },
]

function config(model = "gpt-5.6-sol", thinking = "max") {
  return {
    defaultProvider: "openai",
    defaultModel: model,
    defaultThinkingLevel: thinking,
    authProviders: ["openai"],
    customProviders: [],
  }
}

async function renderPanel(
  env: Record<string, string> = {},
  expectModel = true,
  enabled = true
) {
  let view!: ReturnType<typeof render>
  await act(async () => {
    view = render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <PiConfigPanel
          agent={{ env, enabled } as unknown as AcpAgentInfo}
          saving={false}
          onSaveEnv={api.onSaveEnv}
          onSaved={async () => {}}
        />
      </NextIntlClientProvider>
    )
  })
  if (expectModel) {
    await waitFor(() =>
      expect(screen.getByPlaceholderText("claude-sonnet-5")).toHaveValue(
        "gpt-5.6-sol"
      )
    )
  }
  return view
}

async function openThinkingPicker() {
  await userEvent.click(screen.getAllByRole("combobox").slice(-1)[0])
}

beforeEach(() => {
  vi.clearAllMocks()
  api.loadPiConfig.mockResolvedValue(config())
  api.listPiModelCapabilities.mockResolvedValue(CAPABILITIES)
  api.acpUpdatePiConfig.mockResolvedValue(undefined)
  api.acpValidatePiCommand.mockResolvedValue({
    found: false,
    resolvedPath: null,
    version: null,
  })
  api.acpPiListTrustEntries.mockResolvedValue([])
  api.onSaveEnv.mockResolvedValue(undefined)
})

describe("Pi built-in model thinking capability", () => {
  it("rehydrates a saved max choice for a model whose catalog declares it", async () => {
    const view = await renderPanel()
    await openThinkingPicker()
    expect(screen.getByRole("option", { name: "Max" })).toBeInTheDocument()
    await userEvent.keyboard("{Escape}")
    await userEvent.click(
      screen.getByRole("button", { name: "Save Pi Config" })
    )
    await waitFor(() =>
      expect(api.acpUpdatePiConfig).toHaveBeenCalledWith(
        expect.objectContaining({
          provider: "openai",
          model: "gpt-5.6-sol",
          thinkingLevel: "max",
        })
      )
    )
    view.unmount()
    await renderPanel()
    await openThinkingPicker()
    expect(screen.getByRole("option", { name: "Max" })).toBeInTheDocument()
  })

  it("does not send saved max after switching to a model without reasoning", async () => {
    await renderPanel()
    fireEvent.change(screen.getByPlaceholderText("claude-sonnet-5"), {
      target: { value: "gpt-4" },
    })
    await screen.findByText(
      "This level isn't in the available list above — pi would clamp it."
    )
    await openThinkingPicker()
    expect(
      screen.queryByRole("option", { name: "Max" })
    ).not.toBeInTheDocument()
    await userEvent.keyboard("{Escape}")
    await userEvent.click(
      screen.getByRole("button", { name: "Save Pi Config" })
    )
    expect(api.acpUpdatePiConfig).not.toHaveBeenCalled()
  })

  it("uses the current model when catalog arrives after a model switch", async () => {
    let resolveCatalog!: (value: typeof CAPABILITIES) => void
    api.listPiModelCapabilities.mockReturnValue(
      new Promise((resolve) => {
        resolveCatalog = resolve
      })
    )
    await renderPanel()
    fireEvent.change(screen.getByPlaceholderText("claude-sonnet-5"), {
      target: { value: "gpt-4" },
    })
    await act(async () => resolveCatalog(CAPABILITIES))
    await openThinkingPicker()
    expect(
      screen.queryByRole("option", { name: "Max" })
    ).not.toBeInTheDocument()
    await userEvent.keyboard("{Escape}")
    await userEvent.click(
      screen.getByRole("button", { name: "Save Pi Config" })
    )
    expect(api.acpUpdatePiConfig).not.toHaveBeenCalled()
  })

  it("fails closed when the Pi catalog query is unavailable", async () => {
    api.listPiModelCapabilities.mockRejectedValue(new Error("pi unavailable"))
    await renderPanel()
    await openThinkingPicker()
    expect(
      screen.queryByRole("option", { name: "Max" })
    ).not.toBeInTheDocument()
    await userEvent.keyboard("{Escape}")
    await userEvent.click(
      screen.getByRole("button", { name: "Save Pi Config" })
    )
    expect(api.acpUpdatePiConfig).not.toHaveBeenCalled()
  })

  it("rechecks the catalog after saving a previously missing provider credential", async () => {
    api.loadPiConfig.mockResolvedValue(config("gpt-5.6-sol", "off"))
    api.listPiModelCapabilities
      .mockResolvedValueOnce([])
      .mockResolvedValueOnce(CAPABILITIES)
    await renderPanel()
    await waitFor(() =>
      expect(api.listPiModelCapabilities).toHaveBeenCalledTimes(1)
    )
    await openThinkingPicker()
    expect(
      screen.queryByRole("option", { name: "Max" })
    ).not.toBeInTheDocument()
    await userEvent.keyboard("{Escape}")
    await userEvent.type(screen.getByPlaceholderText(/saved/), "new-api-key")
    await userEvent.click(
      screen.getByRole("button", { name: "Save Pi Config" })
    )
    await waitFor(() =>
      expect(api.listPiModelCapabilities).toHaveBeenCalledTimes(2)
    )
    await openThinkingPicker()
    expect(screen.getByRole("option", { name: "Max" })).toBeInTheDocument()
  })

  it("reloads native config from the newly selected Pi agent directory", async () => {
    api.loadPiConfig
      .mockResolvedValueOnce(config("gpt-5.6-sol", "max"))
      .mockResolvedValueOnce(config("gpt-4", "off"))
    const view = await renderPanel()
    view.rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <PiConfigPanel
          agent={
            {
              env: { PI_CODING_AGENT_DIR: "/tmp/another-agent" },
              enabled: true,
            } as unknown as AcpAgentInfo
          }
          saving={false}
          onSaveEnv={api.onSaveEnv}
          onSaved={async () => {}}
        />
      </NextIntlClientProvider>
    )
    await waitFor(() => expect(api.loadPiConfig).toHaveBeenCalledTimes(2))
    await waitFor(() =>
      expect(screen.getByPlaceholderText("claude-sonnet-5")).toHaveValue(
        "gpt-4"
      )
    )
  })

  it("ignores a catalog from the old Pi executable after runtime changes", async () => {
    let resolveOld!: (value: typeof CAPABILITIES) => void
    api.listPiModelCapabilities
      .mockReturnValueOnce(
        new Promise((resolve) => {
          resolveOld = resolve
        })
      )
      .mockResolvedValueOnce([])
    const view = await renderPanel()
    view.rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <PiConfigPanel
          agent={
            {
              env: { PI_ACP_PI_COMMAND: "/tmp/other-pi" },
              enabled: true,
            } as unknown as AcpAgentInfo
          }
          saving={false}
          onSaveEnv={api.onSaveEnv}
          onSaved={async () => {}}
        />
      </NextIntlClientProvider>
    )
    await waitFor(() =>
      expect(api.listPiModelCapabilities).toHaveBeenCalledTimes(2)
    )
    await act(async () => resolveOld(CAPABILITIES))
    await openThinkingPicker()
    expect(
      screen.queryByRole("option", { name: "Max" })
    ).not.toBeInTheDocument()
  })
})

describe("Pi adapter runtime minimum", () => {
  it("refreshes Pi model capability after the agent is re-enabled", async () => {
    api.listPiModelCapabilities
      .mockResolvedValueOnce([])
      .mockResolvedValue(CAPABILITIES)
    const view = await renderPanel({}, true, false)
    await waitFor(() =>
      expect(api.listPiModelCapabilities).toHaveBeenCalledTimes(1)
    )
    view.rerender(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <PiConfigPanel
          agent={{ env: {}, enabled: true } as unknown as AcpAgentInfo}
          saving={false}
          onSaveEnv={api.onSaveEnv}
          onSaved={async () => {}}
        />
      </NextIntlClientProvider>
    )
    await waitFor(() =>
      expect(api.listPiModelCapabilities).toHaveBeenCalledTimes(2)
    )
    await openThinkingPicker()
    expect(screen.getByRole("option", { name: "Max" })).toBeInTheDocument()
  })

  it("warns immediately when a saved custom runtime predates pi-acp 0.0.34", async () => {
    api.acpValidatePiCommand.mockImplementation(async (command: string) =>
      command === "/tmp/old-pi"
        ? { found: true, resolvedPath: command, version: "0.80.9" }
        : { found: false, resolvedPath: null, version: null }
    )
    await renderPanel({ PI_ACP_PI_COMMAND: "/tmp/old-pi" })
    expect(api.acpValidatePiCommand).toHaveBeenCalledWith("/tmp/old-pi")
    expect(
      await screen.findByText(/pi-acp 0\.0\.34 requires Pi 0\.81\.0 or newer/)
    ).toBeInTheDocument()
  })

  it("flags known incompatible Pi releases and accepts 0.81.0 or later", () => {
    expect(piRuntimeIsTooOld("0.80.9")).toBe(true)
    expect(piRuntimeIsTooOld("pi 0.81.0")).toBe(false)
    expect(piRuntimeIsTooOld("0.87.1")).toBe(false)
    expect(piRuntimeIsTooOld(null)).toBe(false)
  })
})

describe("Pi runtime path identity", () => {
  it("blocks a newly entered relative Pi agent directory", async () => {
    await renderPanel({ PI_ACP_PI_COMMAND: "/tmp/pi" })
    fireEvent.change(screen.getByPlaceholderText("~/.pi/agent"), {
      target: { value: "./agent" },
    })
    expect(
      await screen.findByText(/absolute Pi agent directory/)
    ).toBeInTheDocument()
    await userEvent.click(screen.getByRole("button", { name: "Save Runtime" }))
    expect(api.onSaveEnv).not.toHaveBeenCalled()
  })

  it("does not read or write native config under a saved relative agent directory", async () => {
    await renderPanel(
      {
        PI_ACP_PI_COMMAND: "/tmp/pi",
        PI_CODING_AGENT_DIR: "./agent",
      },
      false
    )
    expect(
      await screen.findAllByText(/absolute Pi agent directory/)
    ).toHaveLength(2)
    expect(api.loadPiConfig).not.toHaveBeenCalled()
    expect(api.listPiModelCapabilities).not.toHaveBeenCalled()
    expect(api.acpPiListTrustEntries).not.toHaveBeenCalled()
    await userEvent.click(
      screen.getByRole("button", { name: "Save Pi Config" })
    )
    expect(api.acpUpdatePiConfig).not.toHaveBeenCalled()
  })

  it("keeps bare PATH commands and home-relative agent directories valid", async () => {
    await renderPanel({ PI_ACP_PI_COMMAND: "pi" })
    fireEvent.change(screen.getByPlaceholderText("~/.pi/agent"), {
      target: { value: "~/pi-agent" },
    })
    await userEvent.click(screen.getByRole("button", { name: "Save Runtime" }))
    expect(api.onSaveEnv).toHaveBeenCalledWith(
      expect.objectContaining({
        PI_ACP_PI_COMMAND: "pi",
        PI_CODING_AGENT_DIR: "~/pi-agent",
      }),
      true
    )
  })

  it("does not query a saved relative Pi command or save a new one", async () => {
    await renderPanel({ PI_ACP_PI_COMMAND: "./pi-test.sh" })
    expect(
      await screen.findByText(/absolute Pi command path/)
    ).toBeInTheDocument()
    expect(api.listPiModelCapabilities).not.toHaveBeenCalled()
    await userEvent.click(screen.getByRole("button", { name: "Save Runtime" }))
    expect(api.onSaveEnv).not.toHaveBeenCalled()
  })
})
