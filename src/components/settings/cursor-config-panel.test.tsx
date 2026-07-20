import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  buildCursorEnv,
  CursorConfigPanel,
  cursorLoginCommand,
  inferCursorMode,
  isCursorForceEnabled,
} from "./cursor-config-panel"
import {
  acpCursorAuthStatus,
  acpCursorListModels,
  acpUpdateAgentConfig,
} from "@/lib/api"
import type { AcpAgentInfo } from "@/lib/types"
import enMessages from "@/i18n/messages/en.json"

vi.mock("@/lib/api", () => ({
  acpCursorAuthStatus: vi.fn(),
  acpCursorListModels: vi.fn(),
  acpUpdateAgentConfig: vi.fn(),
}))

describe("buildCursorEnv", () => {
  it("API-key mode writes the key + model and always scrubs the dead base URL", () => {
    const env = buildCursorEnv(
      { OTHER: "x", CURSOR_API_BASE_URL: "https://stale" },
      "custom",
      "  sk-key  ",
      "claude-opus-4-8-high",
      true
    )
    expect(env).toEqual({
      OTHER: "x",
      CURSOR_AUTH_MODE: "custom",
      CURSOR_API_KEY: "sk-key",
      CURSOR_MODEL: "claude-opus-4-8-high",
      CURSOR_FORCE: "1",
    })
    // The CLI has no custom endpoint — the base URL is never persisted.
    expect(env).not.toHaveProperty("CURSOR_API_BASE_URL")
  })

  it("API-key mode with a blank key clears it but keeps the mode + unrelated keys", () => {
    const env = buildCursorEnv(
      {
        CURSOR_API_KEY: "old",
        CURSOR_API_BASE_URL: "https://old",
        CURSOR_MODEL: "m",
        CURSOR_FORCE: "1",
        KEEP: "y",
      },
      "custom",
      " ",
      "",
      false
    )
    expect(env).toEqual({ KEEP: "y", CURSOR_AUTH_MODE: "custom" })
  })

  it("subscription mode drops the key + base URL but keeps the model", () => {
    // The key arg is ignored in subscription mode — the launch uses browser
    // login, so shipping a key (or a base URL) would be wrong.
    const env = buildCursorEnv(
      { CURSOR_API_KEY: "old", CURSOR_API_BASE_URL: "https://old", KEEP: "y" },
      "subscription",
      "ignored-key",
      "auto",
      false
    )
    expect(env).toEqual({
      KEEP: "y",
      CURSOR_AUTH_MODE: "subscription",
      CURSOR_MODEL: "auto",
    })
  })
})

describe("inferCursorMode", () => {
  it("prefers the explicit knob over key presence", () => {
    expect(
      inferCursorMode({ CURSOR_AUTH_MODE: "subscription", CURSOR_API_KEY: "k" })
    ).toBe("subscription")
    expect(inferCursorMode({ CURSOR_AUTH_MODE: "custom" })).toBe("custom")
  })

  it("infers custom from a saved API key, else subscription (legacy rows)", () => {
    expect(inferCursorMode({ CURSOR_API_KEY: "k" })).toBe("custom")
    expect(inferCursorMode({})).toBe("subscription")
  })
})

describe("cursorLoginCommand", () => {
  it("quotes a path with whitespace and falls back when absent", () => {
    expect(cursorLoginCommand("/Applications/My App/cursor-agent")).toBe(
      '"/Applications/My App/cursor-agent" login'
    )
    expect(cursorLoginCommand("/usr/local/bin/cursor-agent")).toBe(
      "/usr/local/bin/cursor-agent login"
    )
    expect(cursorLoginCommand(null)).toBe("cursor-agent login")
    expect(cursorLoginCommand("")).toBe("cursor-agent login")
  })
})

describe("isCursorForceEnabled", () => {
  it("accepts 1/true in any case with padding, rejects everything else", () => {
    expect(isCursorForceEnabled({ CURSOR_FORCE: "1" })).toBe(true)
    expect(isCursorForceEnabled({ CURSOR_FORCE: " TRUE " })).toBe(true)
    expect(isCursorForceEnabled({ CURSOR_FORCE: "0" })).toBe(false)
    expect(isCursorForceEnabled({ CURSOR_FORCE: "yes" })).toBe(false)
    expect(isCursorForceEnabled({})).toBe(false)
  })
})

describe("CursorConfigPanel", () => {
  const baseAgent = {
    agent_type: "cursor",
    enabled: true,
    env: {} as Record<string, string>,
    cursor_settings: {
      sandbox_mode: null,
      permissions_allow: [],
      permissions_deny: [],
    },
    cursor_cli_config_json: null,
  }

  function renderPanel(overrides?: {
    env?: Record<string, string>
    onSaveEnv?: ReturnType<typeof vi.fn>
    onSaved?: ReturnType<typeof vi.fn>
    onAffectedSessions?: ReturnType<typeof vi.fn>
  }) {
    const onSaveEnv = overrides?.onSaveEnv ?? vi.fn().mockResolvedValue(0)
    const onSaved = overrides?.onSaved ?? vi.fn()
    const onAffectedSessions = overrides?.onAffectedSessions ?? vi.fn()
    const agent = {
      ...baseAgent,
      env: overrides?.env ?? baseAgent.env,
    } as unknown as AcpAgentInfo
    render(
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <CursorConfigPanel
          agent={agent}
          saving={false}
          onSaveEnv={onSaveEnv}
          onSaved={onSaved}
          onAffectedSessions={onAffectedSessions}
        />
      </NextIntlClientProvider>
    )
    return { onSaveEnv, onSaved, onAffectedSessions }
  }

  const authenticated = {
    installed: true,
    is_authenticated: true,
    raw_status: "authenticated",
    email: "itpkcn@gmail.com",
    membership: null,
    error: null,
    binary_path: "/cache/cursor-agent",
  }

  beforeEach(() => {
    vi.clearAllMocks()
    vi.mocked(acpCursorAuthStatus).mockResolvedValue({
      installed: false,
      is_authenticated: false,
      raw_status: null,
      email: null,
      membership: null,
      error: null,
      binary_path: null,
    })
    vi.mocked(acpCursorListModels).mockResolvedValue({
      models: [],
      default_model: null,
      error: null,
    })
    vi.mocked(acpUpdateAgentConfig).mockResolvedValue(0)
  })

  it("rolls the env back when the rules write fails (API-key mode)", async () => {
    // A saved API key opens the panel in API-key mode. The widening hazard: the
    // env step already persisted (e.g. Run Everything on) but the deny rules
    // never landed — the save must restore the previous env.
    vi.mocked(acpUpdateAgentConfig).mockRejectedValue(new Error("disk full"))
    const originalEnv = { CURSOR_API_KEY: "old-key" }
    const { onSaveEnv, onSaved } = renderPanel({ env: originalEnv })
    await screen.findByText(enMessages.AcpAgentSettings.cursor.authNotInstalled)

    fireEvent.change(
      screen.getByPlaceholderText(
        enMessages.AcpAgentSettings.cursor.apiKeyPlaceholder
      ),
      { target: { value: "new-key" } }
    )
    fireEvent.click(
      screen.getByRole("button", {
        name: enMessages.AcpAgentSettings.cursor.saveConfig,
      })
    )

    await waitFor(() => expect(onSaveEnv).toHaveBeenCalledTimes(2))
    // No CURSOR_API_BASE_URL is ever written; CURSOR_FORCE defaults to "1"
    // (Run Everything) because the saved env never set it.
    expect(onSaveEnv.mock.calls[0][0]).toEqual({
      CURSOR_AUTH_MODE: "custom",
      CURSOR_API_KEY: "new-key",
      CURSOR_FORCE: "1",
    })
    // Rollback restores the exact prior env map.
    expect(onSaveEnv.mock.calls[1][0]).toEqual(originalEnv)
    expect(onSaved).not.toHaveBeenCalled()
  })

  it("blocks an API-key save with no key", async () => {
    const { onSaveEnv } = renderPanel({ env: { CURSOR_AUTH_MODE: "custom" } })
    await screen.findByText(enMessages.AcpAgentSettings.cursor.authNotInstalled)

    fireEvent.click(
      screen.getByRole("button", {
        name: enMessages.AcpAgentSettings.cursor.saveConfig,
      })
    )
    // Validation short-circuits before any persistence.
    await waitFor(() =>
      expect(vi.mocked(acpUpdateAgentConfig)).not.toHaveBeenCalled()
    )
    expect(onSaveEnv).not.toHaveBeenCalled()
  })

  it("subscription mode shows the runnable login command and saves login-only env", async () => {
    vi.mocked(acpCursorAuthStatus).mockResolvedValue({
      installed: true,
      is_authenticated: false,
      raw_status: "unauthenticated",
      email: null,
      membership: null,
      error: null,
      binary_path:
        "/Users/x/Library/Caches/app.codeg/acp-binaries/cursor/dist-package/cursor-agent",
    })
    // Empty env → subscription mode.
    const { onSaveEnv, onSaved } = renderPanel({ env: {} })

    // The login command uses the resolved binary path, not a bare name.
    await screen.findByText(
      "/Users/x/Library/Caches/app.codeg/acp-binaries/cursor/dist-package/cursor-agent login"
    )

    fireEvent.click(
      screen.getByRole("button", {
        name: enMessages.AcpAgentSettings.cursor.saveConfig,
      })
    )

    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1))
    expect(onSaveEnv).toHaveBeenCalledTimes(1)
    // Subscription persists the mode only — no credential. A fresh agent
    // defaults the permission mode to Run Everything (CURSOR_FORCE="1").
    expect(onSaveEnv.mock.calls[0][0]).toEqual({
      CURSOR_AUTH_MODE: "subscription",
      CURSOR_FORCE: "1",
    })
  })

  it("respects an explicit CURSOR_FORCE=0 (Ask before running) instead of defaulting on", async () => {
    // A saved "0" means the user chose Ask — the Run-Everything default must
    // not override it, so the save drops CURSOR_FORCE rather than writing "1".
    const { onSaveEnv, onSaved } = renderPanel({
      env: { CURSOR_AUTH_MODE: "subscription", CURSOR_FORCE: "0" },
    })
    await screen.findByText(enMessages.AcpAgentSettings.cursor.authNotInstalled)

    fireEvent.click(
      screen.getByRole("button", {
        name: enMessages.AcpAgentSettings.cursor.saveConfig,
      })
    )

    await waitFor(() => expect(onSaved).toHaveBeenCalledTimes(1))
    expect(onSaveEnv.mock.calls[0][0]).toEqual({
      CURSOR_AUTH_MODE: "subscription",
    })
  })

  it("subscription mode probes with an empty key to use browser login", async () => {
    renderPanel({ env: {} })
    await waitFor(() => expect(acpCursorAuthStatus).toHaveBeenCalled())
    // An empty string forces the login credential and strips any inherited key.
    expect(acpCursorAuthStatus).toHaveBeenCalledWith("")
  })

  it("hides the model picker (and hints to sign in) when no models are fetched", async () => {
    // Authenticated, but the models probe returns nothing → no picker.
    vi.mocked(acpCursorAuthStatus).mockResolvedValue(authenticated)
    vi.mocked(acpCursorListModels).mockResolvedValue({
      models: [],
      default_model: null,
      error: null,
    })
    renderPanel({ env: { CURSOR_AUTH_MODE: "subscription" } })

    await screen.findByText(enMessages.AcpAgentSettings.cursor.modelsNeedAuth)
    expect(
      screen.queryByText(enMessages.AcpAgentSettings.cursor.modelTitle)
    ).toBeNull()
  })

  it("shows the model picker once real models load", async () => {
    vi.mocked(acpCursorAuthStatus).mockResolvedValue(authenticated)
    vi.mocked(acpCursorListModels).mockResolvedValue({
      models: [
        { id: "auto", label: "Auto", is_default: true },
        { id: "claude-opus-4-8-high", label: "Opus 4.8 1M", is_default: false },
      ],
      default_model: "auto",
      error: null,
    })
    renderPanel({ env: { CURSOR_AUTH_MODE: "subscription" } })

    // The picker card (with its header) appears after the catalog loads.
    await screen.findByText(enMessages.AcpAgentSettings.cursor.modelTitle)
  })
})
