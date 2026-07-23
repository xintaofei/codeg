import { describe, expect, it } from "vitest"

import {
  applyClaudeProviderToConfigText,
  buildGrokSaveOptions,
  buildGrokStructuredConfig,
  buildVersionCheck,
  configTextForClaudeSave,
  getAgentChecks,
  inferGrokMode,
  patchImportantConfigText,
} from "./acp-agent-settings"
import type { AcpAgentInfo, AgentType, PreflightResult } from "@/lib/types"

function makeAgent(overrides: Partial<AcpAgentInfo>): AcpAgentInfo {
  return {
    agent_type: "hermes" as AgentType,
    registry_id: "hermes",
    registry_version: "0.16.0",
    name: "Hermes Agent",
    description: "",
    available: true,
    distribution_type: "uvx",
    enabled: true,
    sort_order: 0,
    installed_version: null,
    env: {},
    config_json: null,
    config_file_path: null,
    opencode_auth_json: null,
    codex_auth_json: null,
    cline_secrets_json: null,
    codex_config_toml: null,
    codex_model_catalog: null,
    grok_config_toml: null,
    grok_settings: null,
    hermes_config_yaml: null,
    cursor_cli_config_json: null,
    cursor_settings: null,
    model_provider_id: null,
    ...overrides,
  }
}

// `disabled` lives only on the frontend-synthesized fix variant, not on the
// backend FixAction member of the union — narrow before reading it.
function fixDisabled(fix: unknown): boolean {
  return (
    typeof fix === "object" &&
    fix !== null &&
    "disabled" in fix &&
    (fix as Record<string, unknown>).disabled === true
  )
}

// A full Grok draft, custom-model + compaction fields defaulted empty. Typed
// from the function's own parameter so it stays in sync with the panel shape.
// Defaults to the `custom` auth method so the custom-model save cases below
// exercise the [model.<id>] path; non-custom gating is covered separately.
type GrokDraftInput = Parameters<typeof buildGrokStructuredConfig>[0]
const grokDraft = (
  overrides: Partial<GrokDraftInput> = {}
): GrokDraftInput => ({
  grokAuthMode: "custom",
  grokPermissionMode: "",
  grokReasoningEffort: "",
  grokCustomModelId: "",
  grokCustomBaseUrl: "",
  grokCustomApiKey: "",
  grokCustomApiBackend: "responses",
  grokCustomContextWindow: "",
  grokAutoCompactThreshold: "",
  ...overrides,
})
// The custom-model group when no model id is set (all removed / null).
const emptyCustoms = {
  customModelId: null,
  customBaseUrl: null,
  customApiKey: null,
  customApiBackend: null,
  customContextWindow: null,
  autoCompactThresholdPercent: null,
}

describe("buildGrokStructuredConfig — Grok panel save payload", () => {
  // A chosen dropdown value passes through. These become [ui].permission_mode /
  // [models].default_reasoning_effort on save.
  it("passes chosen values through", () => {
    expect(
      buildGrokStructuredConfig(
        grokDraft({
          grokPermissionMode: "bypassPermissions",
          grokReasoningEffort: "high",
        })
      )
    ).toEqual({
      permissionMode: "bypassPermissions",
      defaultReasoningEffort: "high",
      ...emptyCustoms,
    })
  })

  // The empty "unset / use default" choice maps to null, which the backend
  // treats as "remove this key" — so unsetting a control clears it from
  // config.toml rather than writing an empty string.
  it("maps unset (empty) fields to null", () => {
    expect(buildGrokStructuredConfig(grokDraft())).toEqual({
      permissionMode: null,
      defaultReasoningEffort: null,
      ...emptyCustoms,
    })
  })

  // A custom model id activates the whole [model.<id>] group; api_backend
  // defaults to `responses`, numerics parse, and compaction is independent.
  it("writes the custom model group when a model id is set", () => {
    expect(
      buildGrokStructuredConfig(
        grokDraft({
          grokCustomModelId: "  my-grok  ",
          grokCustomBaseUrl: "https://gw/v1",
          grokCustomApiKey: "xai-123",
          grokCustomApiBackend: "chat_completions",
          grokCustomContextWindow: "131072",
          grokAutoCompactThreshold: "80",
        })
      )
    ).toEqual({
      permissionMode: null,
      defaultReasoningEffort: null,
      customModelId: "my-grok",
      customBaseUrl: "https://gw/v1",
      customApiKey: "xai-123",
      customApiBackend: "chat_completions",
      customContextWindow: 131072,
      autoCompactThresholdPercent: 80,
    })
  })

  // Model-scoped fields are dropped without a model id, but the session-global
  // compaction threshold still parses.
  it("drops model-scoped fields when the model id is empty", () => {
    expect(
      buildGrokStructuredConfig(
        grokDraft({
          grokCustomBaseUrl: "https://gw/v1",
          grokCustomApiKey: "xai-123",
          grokAutoCompactThreshold: "70",
        })
      )
    ).toEqual({
      permissionMode: null,
      defaultReasoningEffort: null,
      ...emptyCustoms,
      autoCompactThresholdPercent: 70,
    })
  })

  // Blank backend with a model set defaults to `responses`; the threshold clamps
  // to 0–100 and a non-positive context window is omitted.
  it("defaults the backend and clamps numeric ranges", () => {
    expect(
      buildGrokStructuredConfig(
        grokDraft({
          grokCustomModelId: "foo",
          grokCustomApiBackend: "",
          grokCustomContextWindow: "0",
          grokAutoCompactThreshold: "150",
        })
      )
    ).toEqual({
      permissionMode: null,
      defaultReasoningEffort: null,
      customModelId: "foo",
      customBaseUrl: null,
      customApiKey: null,
      customApiBackend: "responses",
      customContextWindow: null,
      autoCompactThresholdPercent: 100,
    })
  })

  // The custom-model group is written ONLY in the `custom` auth method. In
  // subscription / api_key mode the [model.<id>] block is dropped even if the
  // (now-hidden) custom fields still hold values — so switching away removes it,
  // while method-independent fields (reasoning effort) still pass through.
  it("drops the custom model when the auth method isn't custom", () => {
    const filled = {
      grokCustomModelId: "my-grok",
      grokCustomBaseUrl: "https://gw/v1",
      grokCustomApiKey: "xai-123",
      grokCustomApiBackend: "chat_completions",
      grokCustomContextWindow: "131072",
    }
    for (const mode of ["subscription", "api_key"] as const) {
      expect(
        buildGrokStructuredConfig(
          grokDraft({
            ...filled,
            grokAuthMode: mode,
            grokReasoningEffort: "high",
          })
        )
      ).toEqual({
        permissionMode: null,
        defaultReasoningEffort: "high",
        ...emptyCustoms,
      })
    }
  })
})

describe("buildGrokSaveOptions — one save persists both surfaces", () => {
  const base = grokDraft({
    grokPermissionMode: "ask",
    grokReasoningEffort: "high",
  })

  // Advanced editor unchanged → send ONLY the structured controls; the backend
  // merges them onto the fresh on-disk config (no stale snapshot to clobber
  // concurrent edits).
  it("omits raw TOML when the advanced editor matches disk", () => {
    const opts = buildGrokSaveOptions(
      { ...base, grokConfigTomlText: "[ui]\n" },
      "[ui]\n"
    )
    expect(opts.grokConfigTomlText).toBeUndefined()
    expect(opts.grokStructured).toEqual({
      permissionMode: "ask",
      defaultReasoningEffort: "high",
      ...emptyCustoms,
    })
  })

  // Advanced editor edited → send it as the merge base so the user's other-key
  // edits are saved TOGETHER with the structured controls in one action (no
  // separate save that could discard either surface).
  it("includes raw TOML when the advanced editor was edited", () => {
    const opts = buildGrokSaveOptions(
      { ...base, grokConfigTomlText: "[ui]\nvim_mode = true\n" },
      "[ui]\n"
    )
    expect(opts.grokConfigTomlText).toBe("[ui]\nvim_mode = true\n")
  })

  // A null on-disk value (no file yet) is treated as empty for the dirty check.
  it("treats null on-disk config as empty", () => {
    expect(
      buildGrokSaveOptions({ ...base, grokConfigTomlText: "" }, null)
        .grokConfigTomlText
    ).toBeUndefined()
    expect(
      buildGrokSaveOptions({ ...base, grokConfigTomlText: "x" }, null)
        .grokConfigTomlText
    ).toBe("x")
  })
})

describe("inferGrokMode — Grok auth-method recognition", () => {
  // An explicit knob always wins, even against a present/absent key.
  it("honors an explicit GROK_AUTH_MODE knob", () => {
    expect(inferGrokMode({ GROK_AUTH_MODE: "subscription" })).toBe(
      "subscription"
    )
    expect(inferGrokMode({ GROK_AUTH_MODE: "api_key", XAI_API_KEY: "" })).toBe(
      "api_key"
    )
    // Subscription wins even if a stale key lingers in env.
    expect(
      inferGrokMode({ GROK_AUTH_MODE: "subscription", XAI_API_KEY: "xai-1" })
    ).toBe("subscription")
  })

  // Legacy rows (no knob): a saved key implies api_key, else subscription.
  it("falls back to XAI_API_KEY presence for legacy rows", () => {
    expect(inferGrokMode({ XAI_API_KEY: "xai-abc" })).toBe("api_key")
    expect(inferGrokMode({ XAI_API_KEY: "   " })).toBe("subscription")
    expect(inferGrokMode({})).toBe("subscription")
  })

  // An unrecognized knob value is ignored, falling through to key presence.
  it("ignores an unknown knob value", () => {
    expect(inferGrokMode({ GROK_AUTH_MODE: "bogus" })).toBe("subscription")
    expect(
      inferGrokMode({ GROK_AUTH_MODE: "bogus", XAI_API_KEY: "xai-1" })
    ).toBe("api_key")
  })

  // The custom-endpoint method: an explicit knob wins; else a configured custom
  // model (in config.toml, surfaced via the hasCustomModel flag) implies it and
  // outranks a lingering key.
  it("recognizes the custom endpoint method", () => {
    expect(inferGrokMode({ GROK_AUTH_MODE: "custom" })).toBe("custom")
    expect(inferGrokMode({}, true)).toBe("custom")
    expect(inferGrokMode({ XAI_API_KEY: "xai-1" }, true)).toBe("custom")
    // An explicit knob still wins over the custom-model flag.
    expect(inferGrokMode({ GROK_AUTH_MODE: "api_key" }, true)).toBe("api_key")
  })
})

describe("buildVersionCheck", () => {
  // uv runtime not ready: a uvx agent (Hermes) must surface a blocked
  // version-status with the agent-install action DISABLED — the actual install
  // happens via the separate "Install uv" preflight action, not here.
  it("blocks the agent-install action for a uvx agent when uv isn't ready", () => {
    const check = buildVersionCheck(
      makeAgent({
        agent_type: "hermes" as AgentType,
        distribution_type: "uvx",
        available: false,
      }),
      false // uvReady
    )

    expect(check?.status).toBe("warn")
    expect(check?.fixes).toHaveLength(1)
    expect(check?.fixes[0].kind).toBe("install_npx")
    expect(fixDisabled(check!.fixes[0])).toBe(true)
  })

  // A prepared package must stay removable even when uv is missing — uninstall
  // only clears the prepared marker and needs no uv.
  it("keeps Uninstall available for a prepared uvx agent when uv isn't ready", () => {
    const check = buildVersionCheck(
      makeAgent({
        distribution_type: "uvx",
        available: false,
        installed_version: "0.16.0",
      }),
      false // uvReady
    )

    expect(check?.status).toBe("warn")
    const installFix = check?.fixes.find((fix) => fix.kind === "install_npx")
    const uninstallFix = check?.fixes.find(
      (fix) => fix.kind === "uninstall_npx"
    )
    expect(installFix).toBeDefined()
    expect(fixDisabled(installFix!)).toBe(true)
    expect(uninstallFix).toBeDefined()
    expect(fixDisabled(uninstallFix!)).toBe(false)
  })

  // uv ready, package not yet prepared: the agent-install action is offered and
  // enabled (this is the prewarm step).
  it("offers an enabled install action for an uv-ready, not-installed uvx agent", () => {
    const check = buildVersionCheck(
      makeAgent({
        distribution_type: "uvx",
        available: true,
        installed_version: null,
      }),
      true // uvReady
    )

    expect(check?.status).toBe("fail")
    const installFix = check?.fixes.find((fix) => fix.kind === "install_npx")
    expect(installFix).toBeDefined()
    expect(fixDisabled(installFix!)).toBe(false)
  })

  // A uvx agent is never platform-unsupported (uvx runs everywhere) — even when
  // unavailable + uv treated ready (no preflight result), it must NOT produce
  // the dead-end platform-unsupported message.
  it("never shows platform-unsupported for a uvx agent", () => {
    const check = buildVersionCheck(
      makeAgent({ distribution_type: "uvx", available: false }),
      true // uvReady (optimistic, e.g. preflight not loaded)
    )

    expect(check?.fixes.length).toBeGreaterThan(0)
    expect(check?.message).not.toContain("does not support")
  })

  // An unavailable binary agent genuinely has no binary for this platform, so
  // the dead-end platform-unsupported state (no fixes) is correct there.
  it("keeps the no-fix platform-unsupported state for an unavailable binary agent", () => {
    const check = buildVersionCheck(
      makeAgent({
        agent_type: "codex" as AgentType,
        distribution_type: "binary",
        available: false,
      })
    )

    expect(check?.status).toBe("fail")
    expect(check?.fixes).toHaveLength(0)
  })
})

describe("getAgentChecks uv gating", () => {
  const uvMissingPreflight: { result: PreflightResult } = {
    result: {
      agent_type: "hermes" as AgentType,
      agent_name: "Hermes Agent",
      passed: false,
      checks: [
        {
          check_id: "uv_available",
          label: "uv",
          status: "fail",
          message: "uv is not installed",
          fixes: [{ label: "Install uv", kind: "install_uv", payload: "" }],
        },
      ],
    },
  }

  // When uv is confirmed missing, the version-status install is blocked AND the
  // actionable "Install uv" fix is present in the same result — never a dead end.
  it("pairs the blocked install with an Install-uv fix when uv is missing", () => {
    const checks = getAgentChecks(
      makeAgent({ distribution_type: "uvx", available: false }),
      uvMissingPreflight
    )

    const versionCheck = checks.find((c) => c.check_id === "version_status")
    expect(versionCheck?.status).toBe("warn")
    expect(fixDisabled(versionCheck!.fixes[0])).toBe(true)

    const hasInstallUv = checks.some((c) =>
      c.fixes.some((fix) => fix.kind === "install_uv")
    )
    expect(hasInstallUv).toBe(true)
  })

  // With no preflight result yet (or an errored one), don't block: that would
  // disable install while the Install-uv button is absent. Show an actionable
  // install instead.
  it("does not block (no dead end) when there is no preflight result", () => {
    const checks = getAgentChecks(
      makeAgent({
        distribution_type: "uvx",
        available: false,
        installed_version: null,
      }),
      undefined
    )

    const versionCheck = checks.find((c) => c.check_id === "version_status")
    expect(versionCheck?.fixes.length).toBeGreaterThan(0)
    const installFix = versionCheck?.fixes.find(
      (fix) => fix.kind === "install_npx"
    )
    expect(installFix).toBeDefined()
    expect(fixDisabled(installFix!)).toBe(false)
  })
})

describe("patchImportantConfigText — Claude custom model option", () => {
  const CLAUDE = "claude_code" as AgentType

  function envOf(configText: string): Record<string, string> {
    if (!configText.trim()) return {}
    const parsed = JSON.parse(configText) as { env?: Record<string, string> }
    return parsed.env ?? {}
  }

  // Binding a Model Provider writes the provider's custom model option into
  // config.env. handleModelProviderSelect forwards the provider's trio values
  // through the patch (authoritative like the five model fields): a defined
  // value is written, an empty/omitted one clears the key.
  it("writes the provider's custom model option trio carried in the bind patch", () => {
    const configText = JSON.stringify({
      env: {
        ANTHROPIC_CUSTOM_MODEL_OPTION: "old/opus",
        ANTHROPIC_CUSTOM_MODEL_OPTION_NAME: "Old Opus",
        ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION: "stale",
      },
    })

    const { configText: next } = patchImportantConfigText(CLAUDE, configText, {
      // The provider supplies both the model fields and the custom option's
      // trio; the bind path forwards them so they overwrite config.env.
      claudeMainModel: "provider-main",
      claudeCustomModelOption: "gw/opus",
      claudeCustomModelOptionName: "GW Opus",
      claudeCustomModelOptionDescription: "via gateway",
    })

    const env = envOf(next)
    expect(env.ANTHROPIC_CUSTOM_MODEL_OPTION).toBe("gw/opus")
    expect(env.ANTHROPIC_CUSTOM_MODEL_OPTION_NAME).toBe("GW Opus")
    expect(env.ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION).toBe("via gateway")
    expect(env.ANTHROPIC_MODEL).toBe("provider-main")
  })

  // Binding a provider that defines no custom option must clear a stale one.
  // handleModelProviderSelect forwards empty strings in that case; both empty
  // and omitted patch values delete the key from config.env.
  it("clears the custom model option when the provider defines none", () => {
    const configText = JSON.stringify({
      env: { ANTHROPIC_CUSTOM_MODEL_OPTION: "gw/opus" },
    })

    // Empty string => authoritative clear (what the bind path sends when
    // claudeModel.customOption is absent).
    const { configText: viaEmpty } = patchImportantConfigText(
      CLAUDE,
      configText,
      {
        claudeMainModel: "provider-main",
        claudeCustomModelOption: "",
        claudeCustomModelOptionName: "",
        claudeCustomModelOptionDescription: "",
      }
    )
    expect(viaEmpty).not.toContain("ANTHROPIC_CUSTOM_MODEL_OPTION")
    expect(envOf(viaEmpty).ANTHROPIC_MODEL).toBe("provider-main")

    // Omitted (undefined) likewise deletes the key.
    const { configText: viaOmit } = patchImportantConfigText(
      CLAUDE,
      configText,
      { claudeMainModel: "provider-main" }
    )
    expect(viaOmit).not.toContain("ANTHROPIC_CUSTOM_MODEL_OPTION")
  })
})

describe("applyClaudeProviderToConfigText — provider-bound stale config", () => {
  function envOf(configText: string): Record<string, string> {
    const parsed = JSON.parse(configText) as { env?: Record<string, string> }
    return parsed.env ?? {}
  }

  // Regression for the config-management save path: a provider bound in an
  // earlier session can leave ANTHROPIC_CUSTOM_MODEL_OPTION* in the on-disk
  // config loaded into configText (handleModelProviderSelect only rewrites it on
  // dropdown change, not on reload). Saving must not persist that stale value —
  // re-deriving from the provider (which omits the custom option) clears it while
  // keeping the provider's model and unrelated keys.
  it("clears a stale custom model option absent from the bound provider", () => {
    const staleConfig = JSON.stringify({
      env: {
        ANTHROPIC_CUSTOM_MODEL_OPTION: "gw/opus-stale",
        ANTHROPIC_CUSTOM_MODEL_OPTION_NAME: "Stale",
        ANTHROPIC_MODEL: "old-main",
        CUSTOM_KEY: "keep-me",
      },
    })

    const next = applyClaudeProviderToConfigText(staleConfig, {
      api_url: "https://gw.example/v1",
      api_key: "sk-x",
      model: JSON.stringify({ main: "prov-main" }), // provider omits the trio
    })

    const env = envOf(next)
    expect(env.ANTHROPIC_CUSTOM_MODEL_OPTION).toBeUndefined()
    expect(env.ANTHROPIC_CUSTOM_MODEL_OPTION_NAME).toBeUndefined()
    expect(env.ANTHROPIC_MODEL).toBe("prov-main") // provider authoritative
    expect(env.ANTHROPIC_BASE_URL).toBe("https://gw.example/v1")
    expect(env.CUSTOM_KEY).toBe("keep-me") // unrelated key preserved
  })

  // When the provider DOES define a custom option, it is written through.
  it("writes the provider's custom model option through", () => {
    const next = applyClaudeProviderToConfigText("", {
      api_url: "https://gw.example/v1",
      api_key: "sk-x",
      model: JSON.stringify({
        main: "prov-main",
        customOption: "gw/opus-preview",
        customOptionName: "GW Opus",
        customOptionDescription: "via gateway",
      }),
    })

    const env = envOf(next)
    expect(env.ANTHROPIC_CUSTOM_MODEL_OPTION).toBe("gw/opus-preview")
    expect(env.ANTHROPIC_CUSTOM_MODEL_OPTION_NAME).toBe("GW Opus")
    expect(env.ANTHROPIC_CUSTOM_MODEL_OPTION_DESCRIPTION).toBe("via gateway")
  })
})

describe("configTextForClaudeSave — bound-Claude save payload", () => {
  const provider = {
    api_url: "https://gw.example/v1",
    api_key: "sk-x",
    model: JSON.stringify({ main: "prov-main" }), // provider omits the trio
  }

  // Bound Claude + valid config: rewrite to provider-authoritative, clearing a
  // stale custom option loaded from disk.
  it("rewrites valid bound-Claude config to be provider-authoritative", () => {
    const stale = JSON.stringify({
      env: {
        ANTHROPIC_CUSTOM_MODEL_OPTION: "gw/stale",
        ANTHROPIC_MODEL: "old-main",
      },
    })
    const next = configTextForClaudeSave(
      stale,
      "claude_code" as AgentType,
      7,
      provider
    )
    const env = (JSON.parse(next) as { env: Record<string, string> }).env
    expect(env.ANTHROPIC_CUSTOM_MODEL_OPTION).toBeUndefined()
    expect(env.ANTHROPIC_MODEL).toBe("prov-main")
  })

  // Regression (round 4): INVALID config JSON must pass through UNCHANGED so
  // persistConfig surfaces the parse error — otherwise patchImportantConfigText
  // would silently recover it to `{}` and persist provider-derived config over
  // the user's broken edits.
  it("passes invalid config through unchanged so the save surfaces the error", () => {
    const invalid = "{ not valid json"
    expect(
      configTextForClaudeSave(invalid, "claude_code" as AgentType, 7, provider)
    ).toBe(invalid)
  })

  // Non-Claude or unbound agents are never rewritten.
  it("leaves config untouched for unbound or non-Claude agents", () => {
    const cfg = JSON.stringify({ env: { FOO: "bar" } })
    expect(
      configTextForClaudeSave(cfg, "claude_code" as AgentType, null, undefined)
    ).toBe(cfg)
    expect(
      configTextForClaudeSave(cfg, "codex" as AgentType, 7, provider)
    ).toBe(cfg)
  })
})
