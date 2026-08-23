import { fireEvent, render, screen, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

import {
  ANTIGRAVITY_ENV_KEYS,
  AntigravityConfigPanel,
  antigravityIncompleteReason,
  buildAntigravityEnv,
  inferAntigravityMethod,
  type AntigravityAuthMethod,
  type AntigravityFormValues,
} from "./antigravity-config-panel"
import type { AcpAgentInfo } from "@/lib/types"
import enMessages from "@/i18n/messages/en.json"

function values(
  method: AntigravityAuthMethod,
  overrides: Partial<AntigravityFormValues> = {}
): AntigravityFormValues {
  return {
    method,
    geminiApiKey: "",
    googleApiKey: "",
    gcpProject: "",
    gcpLocation: "",
    ...overrides,
  }
}

describe("inferAntigravityMethod", () => {
  it("defaults to the browser login and rejects anything unrecognized", () => {
    // The default matters: it is the one path that needs no credential, so a
    // row with nothing recorded still produces a workable settings.json.
    expect(inferAntigravityMethod({})).toBe("oauth-personal")
    expect(inferAntigravityMethod({ AGY_AUTH_METHOD: "vertex-ai" })).toBe(
      "oauth-personal"
    )
    expect(inferAntigravityMethod({ AGY_AUTH_METHOD: "" })).toBe(
      "oauth-personal"
    )
    expect(inferAntigravityMethod({ AGY_AUTH_METHOD: "agent-platform" })).toBe(
      "agent-platform"
    )
  })
})

describe("buildAntigravityEnv", () => {
  it("keeps only the credential the chosen method uses and preserves the rest", () => {
    // Switching away from API-key auth must DELETE the key. Leaving it would
    // authenticate as something the user did not pick and would contradict the
    // auth.type the same launch writes to settings.json.
    const stale = {
      KEEP: "y",
      GEMINI_API_KEY: "old",
      GOOGLE_API_KEY: "older",
      GOOGLE_CLOUD_PROJECT: "p",
      GOOGLE_CLOUD_LOCATION: "global",
    }
    expect(buildAntigravityEnv(stale, values("oauth-personal"))).toEqual({
      KEEP: "y",
      AGY_AUTH_METHOD: "oauth-personal",
    })

    expect(
      buildAntigravityEnv(
        stale,
        values("gemini-api-key", { geminiApiKey: "  AIza-new  " })
      )
    ).toEqual({
      KEEP: "y",
      AGY_AUTH_METHOD: "gemini-api-key",
      GEMINI_API_KEY: "AIza-new",
    })
  })

  it("gives Gemini Enterprise its project and location", () => {
    // oauth-business reads gcp.project/location from settings.json only, and
    // the launch path copies them out of these very env values.
    expect(
      buildAntigravityEnv(
        {},
        values("oauth-business", { gcpProject: "acme", gcpLocation: "eu" })
      )
    ).toEqual({
      AGY_AUTH_METHOD: "oauth-business",
      GOOGLE_CLOUD_PROJECT: "acme",
      GOOGLE_CLOUD_LOCATION: "eu",
    })
  })

  it("lets an Agent Platform API key suppress the project and location", () => {
    // Matches the server: when GOOGLE_API_KEY is set its `_vertex_config`
    // suppresses both, so storing all three would keep two values that can
    // never take effect.
    expect(
      buildAntigravityEnv(
        {},
        values("agent-platform", {
          googleApiKey: "AIza-vertex",
          gcpProject: "ignored",
          gcpLocation: "ignored",
        })
      )
    ).toEqual({
      AGY_AUTH_METHOD: "agent-platform",
      GOOGLE_API_KEY: "AIza-vertex",
    })

    expect(
      buildAntigravityEnv(
        {},
        values("agent-platform", { gcpProject: "p", gcpLocation: "us" })
      )
    ).toEqual({
      AGY_AUTH_METHOD: "agent-platform",
      GOOGLE_CLOUD_PROJECT: "p",
      GOOGLE_CLOUD_LOCATION: "us",
    })
  })

  it("writes every key the settings page has to fold into the raw draft", () => {
    // ANTIGRAVITY_ENV_KEYS is what `persistEnv`'s draftEnvPatch iterates; a key
    // this panel writes but that list omits would be silently dropped the next
    // time the enable switch persists the draft wholesale.
    const written = buildAntigravityEnv(
      {},
      values("agent-platform", { gcpProject: "p", gcpLocation: "us" })
    )
    for (const key of Object.keys(written)) {
      expect(ANTIGRAVITY_ENV_KEYS).toContain(key)
    }
  })
})

describe("antigravityIncompleteReason", () => {
  it("mirrors the server's own auth_required rules", () => {
    // Browser login needs nothing — the server opens a browser at session/new.
    expect(antigravityIncompleteReason(values("oauth-personal"))).toBeNull()

    expect(antigravityIncompleteReason(values("gemini-api-key"))).toBe(
      "missingGeminiApiKey"
    )
    expect(
      antigravityIncompleteReason(
        values("gemini-api-key", { geminiApiKey: "AIza" })
      )
    ).toBeNull()

    // Agent Platform: a key OR both a project and a location.
    expect(antigravityIncompleteReason(values("agent-platform"))).toBe(
      "missingAgentPlatformConfig"
    )
    expect(
      antigravityIncompleteReason(values("agent-platform", { gcpProject: "p" }))
    ).toBe("missingAgentPlatformConfig")
    expect(
      antigravityIncompleteReason(
        values("agent-platform", { gcpProject: "p", gcpLocation: "us" })
      )
    ).toBeNull()
    expect(
      antigravityIncompleteReason(
        values("agent-platform", { googleApiKey: "AIza" })
      )
    ).toBeNull()

    // Gemini Enterprise needs BOTH; the server falls through to its no-config
    // path on a partial one.
    expect(
      antigravityIncompleteReason(
        values("oauth-business", { gcpProject: "acme" })
      )
    ).toBe("missingEnterpriseConfig")
    expect(
      antigravityIncompleteReason(
        values("oauth-business", { gcpProject: "acme", gcpLocation: "eu" })
      )
    ).toBeNull()
  })
})

describe("AntigravityConfigPanel", () => {
  const m = enMessages.AcpAgentSettings.antigravity

  type PanelOverrides = {
    env?: Record<string, string>
    onSaveEnv?: ReturnType<typeof vi.fn>
    onSaved?: ReturnType<typeof vi.fn>
  }

  function agentOf(overrides?: PanelOverrides): AcpAgentInfo {
    return {
      agent_type: "antigravity",
      enabled: true,
      env: overrides?.env ?? {},
      config_json: null,
      config_file_path: "/home/u/.gemini/antigravity-acp/settings.json",
    } as unknown as AcpAgentInfo
  }

  function renderPanel(overrides?: PanelOverrides) {
    const onSaveEnv = overrides?.onSaveEnv ?? vi.fn().mockResolvedValue(0)
    const onSaved = overrides?.onSaved ?? vi.fn()
    const tree = (agent: AcpAgentInfo) => (
      <NextIntlClientProvider locale="en" messages={enMessages}>
        <AntigravityConfigPanel
          agent={agent}
          saving={false}
          onSaveEnv={onSaveEnv}
          onSaved={onSaved}
        />
      </NextIntlClientProvider>
    )
    const view = render(tree(agentOf(overrides)))
    return {
      onSaveEnv,
      onSaved,
      rerender: (next: PanelOverrides) =>
        view.rerender(tree(agentOf({ ...overrides, ...next }))),
    }
  }

  beforeEach(() => {
    vi.clearAllMocks()
  })

  it("shows the browser-login hint and the settings.json path by default", () => {
    renderPanel()
    // The hint has to say a browser opens: that is the whole sign-in flow for
    // this method, and it happens at the first new session rather than here.
    expect(
      screen.getByText(m.methodHints["oauth-personal"])
    ).toBeInTheDocument()
    expect(
      screen.getByText("/home/u/.gemini/antigravity-acp/settings.json")
    ).toBeInTheDocument()
    // Nothing to fill in, so no warning.
    expect(screen.queryByText(m.missingGeminiApiKey)).not.toBeInTheDocument()
  })

  it("warns when the persisted method has no usable credential", () => {
    renderPanel({ env: { AGY_AUTH_METHOD: "gemini-api-key" } })
    expect(screen.getByText(m.missingGeminiApiKey)).toBeInTheDocument()
  })

  it("saves the credential the persisted method uses and drops the others", async () => {
    const onSaveEnv = vi.fn().mockResolvedValue(0)
    const onSaved = vi.fn()
    renderPanel({
      env: {
        AGY_AUTH_METHOD: "gemini-api-key",
        GEMINI_API_KEY: "old",
        GOOGLE_API_KEY: "stale",
        KEEP: "y",
      },
      onSaveEnv,
      onSaved,
    })

    fireEvent.change(screen.getByLabelText(m.geminiApiKeyLabel), {
      target: { value: "AIza-new" },
    })
    fireEvent.click(screen.getByRole("button", { name: m.save }))

    await waitFor(() => expect(onSaveEnv).toHaveBeenCalledTimes(1))
    expect(onSaveEnv).toHaveBeenCalledWith(
      {
        KEEP: "y",
        AGY_AUTH_METHOD: "gemini-api-key",
        GEMINI_API_KEY: "AIza-new",
      },
      true
    )
    await waitFor(() => expect(onSaved).toHaveBeenCalled())
  })

  it("keeps an in-progress edit when the persisted row changes underneath", () => {
    const { rerender } = renderPanel({
      env: { AGY_AUTH_METHOD: "gemini-api-key", GEMINI_API_KEY: "old" },
    })
    const input = screen.getByLabelText(m.geminiApiKeyLabel)
    fireEvent.change(input, { target: { value: "typing-in-progress" } })

    // The settings page refetches after any save (possibly one from another
    // panel); a re-seed here would throw away what the user is typing.
    rerender({
      env: { AGY_AUTH_METHOD: "gemini-api-key", GEMINI_API_KEY: "x" },
    })
    expect((input as HTMLInputElement).value).toBe("typing-in-progress")
  })

  it("re-seeds from the persisted row while the form is untouched", () => {
    const { rerender } = renderPanel({
      env: { AGY_AUTH_METHOD: "oauth-business", GOOGLE_CLOUD_PROJECT: "acme" },
    })
    expect(
      (screen.getByLabelText(m.gcpProjectLabel) as HTMLInputElement).value
    ).toBe("acme")

    rerender({
      env: {
        AGY_AUTH_METHOD: "oauth-business",
        GOOGLE_CLOUD_PROJECT: "acme-2",
      },
    })
    expect(
      (screen.getByLabelText(m.gcpProjectLabel) as HTMLInputElement).value
    ).toBe("acme-2")
  })
})
