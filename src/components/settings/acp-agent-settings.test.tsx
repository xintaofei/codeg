import { describe, expect, it } from "vitest"

import {
  applyClaudeProviderToConfigText,
  buildCodexSandboxConfig,
  codexSandboxBaselineOf,
  codexSandboxSaveConfig,
  buildGrokSaveOptions,
  buildGrokStructuredConfig,
  buildMergeConfigPayload,
  buildAcpAdapterCheck,
  buildVersionCheck,
  configTextForClaudeSave,
  getAgentChecks,
  hostToolsAgentModeEnabled,
  importantEnvKeysByAgent,
  importantFieldsFor,
  inferGrokMode,
  materializeClaudeHardeningFlags,
  patchCodexConfigTomlText,
  patchEnvByImportantKey,
  patchImportantConfigText,
  rebaseDeepSeekDraft,
  setClaudeEnvFlagInConfigText,
  setHostToolsAgentMode,
} from "./acp-agent-settings"
import { parse as parseTomlDocument } from "smol-toml"
import type {
  AcpAgentInfo,
  AdapterInfo,
  AgentType,
  PreflightResult,
} from "@/lib/types"

function makeAgent(overrides: Partial<AcpAgentInfo>): AcpAgentInfo {
  return {
    agent_type: "hermes" as AgentType,
    skills_capable: true,
    registry_id: "hermes",
    registry_version: "0.16.0",
    name: "Hermes Agent",
    description: "",
    available: true,
    distribution_type: "uvx",
    is_acp_adapter: false,
    custom_source: null,
    enabled: true,
    sort_order: 0,
    installed_version: null,
    host_tools_agent_mode: false,
    env: {},
    config_json: null,
    config_file_path: null,
    opencode_auth_json: null,
    codex_auth_json: null,
    cline_secrets_json: null,
    codex_config_toml: null,
    codex_model_catalog: null,
    codex_sandbox_settings: null,
    grok_config_toml: null,
    grok_settings: null,
    hermes_config_yaml: null,
    cursor_cli_config_json: null,
    cursor_settings: null,
    model_provider_id: null,
    icon_url: null,
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

type CodexSandboxDraft = Parameters<typeof buildCodexSandboxConfig>[0]

const CODEX_GRANULAR_SEED = {
  sandbox_approval: true,
  rules: true,
  skill_approval: false,
  request_permissions: false,
  mcp_elicitations: true,
}

/** A draft whose baseline equals its current values — i.e. "nothing touched
 * yet", exactly what `buildAgentDraft` produces from a freshly-read config. */
function codexSandboxDraft(
  disk: Partial<CodexSandboxDraft> = {},
  edits: Partial<CodexSandboxDraft> = {}
): CodexSandboxDraft {
  const seeded = {
    codexApprovalPolicy: "" as CodexSandboxDraft["codexApprovalPolicy"],
    codexGranular: CODEX_GRANULAR_SEED,
    codexSandboxMode: "" as CodexSandboxDraft["codexSandboxMode"],
    codexWritableRootsText: "",
    codexNetworkAccess: false,
    codexExcludeTmpdirEnvVar: false,
    codexExcludeSlashTmp: false,
    ...disk,
  }
  return {
    ...seeded,
    ...edits,
    codexSandboxBaseline: codexSandboxBaselineOf(seeded),
  }
}

describe("buildCodexSandboxConfig — Codex sandbox/approval save patch", () => {
  // The core contract. The panel also sends the raw config.toml text and the
  // backend applies this patch last, so a field the user did not move must not
  // appear at all — otherwise a hand-edit in the raw editor gets reverted by
  // the panel's stale value for that key.
  it("sends nothing when no control moved", () => {
    expect(
      buildCodexSandboxConfig(
        codexSandboxDraft({
          codexApprovalPolicy: "never",
          codexSandboxMode: "workspace-write",
          codexWritableRootsText: "/srv/one",
          codexNetworkAccess: true,
        })
      )
    ).toEqual({})
  })

  it("sends only the field that moved", () => {
    expect(
      buildCodexSandboxConfig(
        codexSandboxDraft(
          { codexApprovalPolicy: "never", codexSandboxMode: "read-only" },
          { codexSandboxMode: "danger-full-access" }
        )
      )
    ).toEqual({ sandboxMode: "danger-full-access" })
  })

  // Clearing a control back to "not set" must reach the backend as an explicit
  // null (remove the key), not as an absent field (leave it alone).
  it("clears a control with an explicit null", () => {
    expect(
      buildCodexSandboxConfig(
        codexSandboxDraft(
          { codexSandboxMode: "never" as never },
          {
            codexSandboxMode: "",
          }
        )
      )
    ).toEqual({ sandboxMode: null })
  })

  // Approval is one externally tagged key upstream, so both representations
  // travel together whenever either side changes.
  it("moves the approval preset and granular table as a pair", () => {
    const toGranular = buildCodexSandboxConfig(
      codexSandboxDraft(
        { codexApprovalPolicy: "never" },
        { codexApprovalPolicy: "granular" }
      )
    )
    expect(toGranular).toEqual({
      approvalPolicy: null,
      granular: CODEX_GRANULAR_SEED,
    })

    const toPreset = buildCodexSandboxConfig(
      codexSandboxDraft(
        { codexApprovalPolicy: "granular" },
        { codexApprovalPolicy: "on-request" }
      )
    )
    expect(toPreset).toEqual({ approvalPolicy: "on-request", granular: null })
  })

  it("detects a flipped granular switch while staying on granular", () => {
    const patch = buildCodexSandboxConfig(
      codexSandboxDraft(
        { codexApprovalPolicy: "granular" },
        {
          codexApprovalPolicy: "granular",
          codexGranular: { ...CODEX_GRANULAR_SEED, rules: false },
        }
      )
    )
    expect(patch.granular).toEqual({ ...CODEX_GRANULAR_SEED, rules: false })
    expect(patch.approvalPolicy).toBeNull()
  })

  // codex only reads the workspace-write group under `workspace-write`, so
  // dormant values are inert — clearing them would destroy the user's roots on
  // a round-trip through read-only or full-access.
  it("does not touch dormant workspace-write values when the mode changes", () => {
    expect(
      buildCodexSandboxConfig(
        codexSandboxDraft(
          {
            codexSandboxMode: "workspace-write",
            codexWritableRootsText: "/srv/one",
            codexNetworkAccess: true,
          },
          { codexSandboxMode: "danger-full-access" }
        )
      )
    ).toEqual({ sandboxMode: "danger-full-access" })
  })

  it("sends a flag toggle without disturbing its siblings", () => {
    expect(
      buildCodexSandboxConfig(
        codexSandboxDraft(
          { codexWritableRootsText: "/srv/one", codexNetworkAccess: true },
          { codexExcludeSlashTmp: true }
        )
      )
    ).toEqual({ excludeSlashTmp: true })
  })

  it("normalizes the roots box and ignores cosmetic whitespace", () => {
    expect(
      buildCodexSandboxConfig(
        codexSandboxDraft(
          { codexWritableRootsText: "/srv/one" },
          { codexWritableRootsText: "  /srv/one  \n\n" }
        )
      )
    ).toEqual({})
    expect(
      buildCodexSandboxConfig(
        codexSandboxDraft(
          {},
          { codexWritableRootsText: " /srv/one \n\n/srv/two\n" }
        )
      )
    ).toEqual({ writableRoots: ["/srv/one", "/srv/two"] })
  })

  // codex does NOT reject a relative writable_roots entry — it resolves it
  // against CODEX_HOME, so "docs" would silently grant write access to
  // ~/.codex/docs. The save must fail loudly instead.
  it("refuses relative writable roots the user just typed", () => {
    expect(() =>
      buildCodexSandboxConfig(
        codexSandboxDraft({}, { codexWritableRootsText: "/srv/ok\ndocs/rel" })
      )
    ).toThrow(/docs\/rel/)
  })

  // A relative root already on disk is not this save's problem: the field is
  // untouched, so it is not in the patch and must not block the save.
  it("tolerates a pre-existing relative root while it stays untouched", () => {
    expect(
      buildCodexSandboxConfig(
        codexSandboxDraft(
          { codexWritableRootsText: "rel/dir" },
          { codexNetworkAccess: true }
        )
      )
    ).toEqual({ networkAccess: true })
  })

  it("accepts POSIX and Windows absolute roots", () => {
    expect(
      buildCodexSandboxConfig(
        codexSandboxDraft(
          {},
          {
            codexWritableRootsText:
              "/srv/one\nC:\\work\\repo\n\\\\server\\share",
          }
        )
      ).writableRoots
    ).toHaveLength(3)
  })
})

describe("codexSandboxSaveConfig — omit the field when nothing moved", () => {
  it("returns undefined for an untouched group", () => {
    expect(
      codexSandboxSaveConfig(
        codexSandboxDraft({ codexApprovalPolicy: "never" })
      )
    ).toBeUndefined()
  })

  it("returns the patch once a control moves", () => {
    expect(
      codexSandboxSaveConfig(
        codexSandboxDraft({}, { codexSandboxMode: "read-only" })
      )
    ).toEqual({ sandboxMode: "read-only" })
  })
})

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

describe("buildAcpAdapterCheck", () => {
  function makeAdapter(overrides: Partial<AdapterInfo> = {}): AdapterInfo {
    return {
      adapter_package: "@agentclientprotocol/claude-agent-acp@0.63.0",
      adapter_cmd: "claude-agent-acp",
      adapter_installed: false,
      native_cmd: "claude",
      native_label: "Claude Code CLI",
      native_path: "/opt/homebrew/bin/claude",
      shared_config_dir: "~/.claude",
      docs_url: "https://docs.codeg.app/guide/supported-agents#acp-adapters",
      ...overrides,
    }
  }

  // Nothing for the ten agents whose registry command IS the vendor CLI, and
  // nothing before preflight resolves — the card must never appear speculatively.
  it("produces nothing for non-adapter agents or before preflight resolves", () => {
    expect(buildAcpAdapterCheck(null)).toBeNull()
    expect(buildAcpAdapterCheck(undefined)).toBeNull()
  })

  // The whole point of the card: the user has `claude`, we say so by path, and
  // name the different thing we actually need.
  it("names the detected CLI, the adapter package and the shared config dir", () => {
    const check = buildAcpAdapterCheck(makeAdapter())
    expect(check?.check_id).toBe("acp_adapter")
    // warn, not fail: Version Status below already fails: this one explains.
    expect(check?.status).toBe("warn")
    expect(check?.message).toContain("/opt/homebrew/bin/claude")
    expect(check?.message).toContain(
      "@agentclientprotocol/claude-agent-acp@0.63.0"
    )
    expect(check?.message).toContain("~/.claude")
  })

  // Undetected vendor CLI is not a dead end — the explanation still stands, it
  // just can't point at a path.
  it("still explains the split when no vendor CLI was found", () => {
    const check = buildAcpAdapterCheck(makeAdapter({ native_path: null }))
    expect(check?.status).toBe("warn")
    expect(check?.message).toContain(
      "@agentclientprotocol/claude-agent-acp@0.63.0"
    )
    expect(check?.message).not.toContain("/opt/homebrew/bin/claude")
  })

  // Installed → pass, so renderCheck collapses it and a working setup isn't
  // nagged by an explainer it no longer needs.
  it("passes once the adapter is installed", () => {
    const check = buildAcpAdapterCheck(makeAdapter({ adapter_installed: true }))
    expect(check?.status).toBe("pass")
    expect(check?.message).toContain("claude-agent-acp")
  })

  // Exactly one action, and it never duplicates the Install button that lives
  // on the Version Status card directly below.
  it("offers only a docs link, never a second install button", () => {
    const check = buildAcpAdapterCheck(makeAdapter())
    expect(check?.fixes).toHaveLength(1)
    expect(check?.fixes[0].kind).toBe("open_url")
    expect(check?.fixes[0].payload).toContain("#acp-adapters")
  })
})

describe("getAgentChecks adapter ordering", () => {
  // The explainer answers "why does this say not installed?" and must be read
  // BEFORE the Version Status card that offers the fix.
  it("puts the adapter card first, then version, then backend checks", () => {
    const checks = getAgentChecks(
      makeAgent({
        agent_type: "claude_code" as AgentType,
        distribution_type: "npx",
        is_acp_adapter: true,
        registry_version: "0.63.0",
        installed_version: null,
      }),
      {
        result: {
          agent_type: "claude_code" as AgentType,
          agent_name: "Claude Code",
          passed: true,
          checks: [
            {
              check_id: "node_available",
              label: "Node.js",
              status: "pass",
              message: "Node.js v22.0.0 available",
              fixes: [],
            },
          ],
          adapter: {
            adapter_package: "@agentclientprotocol/claude-agent-acp@0.63.0",
            adapter_cmd: "claude-agent-acp",
            adapter_installed: false,
            native_cmd: "claude",
            native_label: "Claude Code CLI",
            native_path: "/usr/local/bin/claude",
            shared_config_dir: "~/.claude",
            docs_url:
              "https://docs.codeg.app/guide/supported-agents#acp-adapters",
          },
        },
      }
    )

    expect(checks.map((c) => c.check_id)).toEqual([
      "acp_adapter",
      "version_status",
      "node_available",
    ])
  })

  // A plain agent's list is byte-for-byte what it was before this feature.
  it("adds nothing for an agent with no adapter relation", () => {
    const checks = getAgentChecks(
      makeAgent({ distribution_type: "npx", installed_version: "1.0.0" }),
      {
        result: {
          agent_type: "gemini" as AgentType,
          agent_name: "Gemini CLI",
          passed: true,
          checks: [],
          adapter: null,
        },
      }
    )
    expect(checks.some((c) => c.check_id === "acp_adapter")).toBe(false)
  })
})

describe("buildVersionCheck", () => {
  // A manually written definition has no registry behind it — its stored
  // version is user-typed — so the check must show the local side alone and
  // never manufacture an "upgrade available" against that noise.
  it("shows only the local version for a manually added custom agent", () => {
    const installed = buildVersionCheck(
      makeAgent({
        agent_type: "custom:goose" as AgentType,
        distribution_type: "npx",
        custom_source: "manual",
        registry_version: "0.0.0",
        installed_version: "1.44.0",
      })
    )
    expect(installed?.status).toBe("pass")
    // The manual pass message, not the generic "Already latest" (there is no
    // "latest" to be at). The mocked translator returns raw templates, so
    // assertions stay at the template level.
    expect(installed?.message).toContain("Installed")
    expect(installed?.message).not.toContain("Already latest")
    // The registry-comparison flow would have produced an upgrade hint here
    // (0.0.0 < 1.44.0 is "not latest" in the generic flow's terms).
    expect(installed?.fixes.some((fix) => fix.kind === "upgrade_npx")).toBe(
      false
    )
    expect(installed?.fixes.some((fix) => fix.kind === "uninstall_npx")).toBe(
      true
    )
  })

  it("still demands an install for a manual custom agent with no local version", () => {
    const check = buildVersionCheck(
      makeAgent({
        agent_type: "custom:goose" as AgentType,
        distribution_type: "npx",
        custom_source: "manual",
        registry_version: "0.0.0",
        installed_version: null,
      })
    )
    expect(check?.status).toBe("fail")
    expect(check?.fixes.some((fix) => fix.kind === "install_npx")).toBe(true)
  })

  // A registry-added custom agent keeps the full remote/local comparison — its
  // stored version is a real registry snapshot.
  it("keeps the remote comparison for a registry-added custom agent", () => {
    const check = buildVersionCheck(
      makeAgent({
        agent_type: "custom:goose" as AgentType,
        distribution_type: "npx",
        custom_source: "registry",
        registry_version: "2.0.0",
        installed_version: "1.0.0",
      })
    )
    expect(check?.status).toBe("warn")
    expect(check?.message).toContain("Upgrade available")
    expect(check?.fixes.some((fix) => fix.kind === "upgrade_npx")).toBe(true)
  })

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
      adapter: null,
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

  // The hardening toggles are not provider-controlled, so a provider-authoritative
  // rewrite must leave them intact while it overwrites the model keys.
  it("preserves a hardening env flag through the provider rewrite", () => {
    const withFlag = JSON.stringify({
      env: {
        CLAUDE_CODE_ATTRIBUTION_HEADER: "0",
        ANTHROPIC_MODEL: "old-main",
      },
    })
    const next = applyClaudeProviderToConfigText(withFlag, {
      api_url: "https://gw.example/v1",
      api_key: "sk-x",
      model: JSON.stringify({ main: "prov-main" }),
    })
    const env = envOf(next)
    expect(env.CLAUDE_CODE_ATTRIBUTION_HEADER).toBe("0")
    expect(env.ANTHROPIC_MODEL).toBe("prov-main")
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

describe("setClaudeEnvFlagInConfigText — Claude hardening toggles", () => {
  const KEY = "CLAUDE_CODE_ATTRIBUTION_HEADER"

  function envOf(configText: string): Record<string, string> {
    const parsed = JSON.parse(configText) as { env?: Record<string, string> }
    return parsed.env ?? {}
  }

  it("sets the flag value while preserving other env + root keys", () => {
    const base = JSON.stringify({
      model: "opus",
      env: { ANTHROPIC_BASE_URL: "https://gw" },
    })
    const next = setClaudeEnvFlagInConfigText(base, KEY, "1")
    expect(next.recoveredFromInvalid).toBe(false)
    const parsed = JSON.parse(next.configText) as {
      model?: string
      env?: Record<string, string>
    }
    expect(parsed.model).toBe("opus")
    expect(parsed.env?.[KEY]).toBe("1")
    expect(parsed.env?.ANTHROPIC_BASE_URL).toBe("https://gw")
  })

  it("creates the env when the config has none", () => {
    const next = setClaudeEnvFlagInConfigText("", KEY, "0")
    expect(next.recoveredFromInvalid).toBe(false)
    expect(envOf(next.configText)[KEY]).toBe("0")
  })

  it("overwrites an existing value (off → on) and keeps siblings", () => {
    const base = JSON.stringify({
      env: { [KEY]: "0", ANTHROPIC_MODEL: "keep" },
    })
    const next = setClaudeEnvFlagInConfigText(base, KEY, "1")
    const env = envOf(next.configText)
    expect(env[KEY]).toBe("1")
    expect(env.ANTHROPIC_MODEL).toBe("keep")
  })

  it("recovers from invalid JSON and writes a fresh config", () => {
    const next = setClaudeEnvFlagInConfigText("{ not json", KEY, "1")
    expect(next.recoveredFromInvalid).toBe(true)
    expect(envOf(next.configText)[KEY]).toBe("1")
  })
})

describe("buildMergeConfigPayload — merge-strategy save diff", () => {
  const KEY = "CLAUDE_CODE_ATTRIBUTION_HEADER"

  // Toggling off the only flag empties configText; the diff against the original
  // must still emit env: null so the backend merge deletes it from disk.
  it("nulls a removed env key when the config emptied out", () => {
    const original = JSON.stringify({ env: { [KEY]: "0" } })
    const payload = buildMergeConfigPayload("", original)
    expect(payload).not.toBeNull()
    expect(JSON.parse(payload as string)).toEqual({ env: null })
  })

  it("returns null when both current and original are empty", () => {
    expect(buildMergeConfigPayload("", null)).toBeNull()
    expect(buildMergeConfigPayload("", "")).toBeNull()
  })

  it("nulls only the removed key when siblings remain", () => {
    const original = JSON.stringify({ env: { [KEY]: "0", OTHER: "x" } })
    const current = JSON.stringify({ env: { OTHER: "x" } })
    const payload = buildMergeConfigPayload(current, original)
    expect(JSON.parse(payload as string)).toEqual({
      env: { OTHER: "x", [KEY]: null },
    })
  })

  it("passes a fresh config through with no null patches", () => {
    const current = JSON.stringify({ env: { [KEY]: "0" } })
    const payload = buildMergeConfigPayload(current, null)
    expect(JSON.parse(payload as string)).toEqual({ env: { [KEY]: "0" } })
  })
})

describe("materializeClaudeHardeningFlags — save-time toggle defaults", () => {
  const ATTR = "CLAUDE_CODE_ATTRIBUTION_HEADER"
  const TRAFFIC = "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC"

  function envOf(configText: string): Record<string, string> {
    const parsed = JSON.parse(configText) as { env?: Record<string, string> }
    return parsed.env ?? {}
  }

  // Fresh config + the intended defaults: don't send the header ("0"), disable
  // telemetry ("1") — written into BOTH the native config env and envText.
  it("writes both flags per the defaults into config env and envText", () => {
    const { configText, envText } = materializeClaudeHardeningFlags("", "", {
      sendAttributionHeader: false,
      disableNonessentialTraffic: true,
    })
    const env = envOf(configText)
    expect(env[ATTR]).toBe("0")
    expect(env[TRAFFIC]).toBe("1")
    expect(envText).toContain(`${ATTR}=0`)
    expect(envText).toContain(`${TRAFFIC}=1`)
  })

  it("writes both on when both toggles are enabled, preserving other keys", () => {
    const base = JSON.stringify({ model: "opus", env: { KEEP: "y" } })
    const { configText, envText } = materializeClaudeHardeningFlags(
      base,
      "KEEP=y",
      { sendAttributionHeader: true, disableNonessentialTraffic: true }
    )
    const parsed = JSON.parse(configText) as {
      model?: string
      env?: Record<string, string>
    }
    expect(parsed.model).toBe("opus")
    expect(parsed.env?.[ATTR]).toBe("1")
    expect(parsed.env?.[TRAFFIC]).toBe("1")
    expect(parsed.env?.KEEP).toBe("y")
    expect(envText).toContain("KEEP=y")
    expect(envText).toContain(`${ATTR}=1`)
  })

  // Regression: invalid JSON must be returned UNCHANGED (never recovered to a
  // minimal {env}), so persistConfig rejects it instead of the merge diff
  // deleting every other on-disk key.
  it("leaves invalid config JSON untouched (no recovery, no data loss)", () => {
    const invalid = "{ not valid json"
    const { configText, envText } = materializeClaudeHardeningFlags(
      invalid,
      "EXISTING=1",
      { sendAttributionHeader: false, disableNonessentialTraffic: true }
    )
    expect(configText).toBe(invalid)
    expect(envText).toBe("EXISTING=1")
  })
})

describe("patchCodexConfigTomlText — codeg's requires_openai_auth default", () => {
  /** Read `model_providers.codeg.requires_openai_auth` back out of a result. */
  function authFlagOf(configTomlText: string): boolean | undefined {
    const parsed = parseTomlDocument(configTomlText) as {
      model_providers?: Record<string, { requires_openai_auth?: unknown }>
    }
    const value = parsed.model_providers?.codeg?.requires_openai_auth
    return typeof value === "boolean" ? value : undefined
  }

  // The three structured controls that reach ensureCodexProviderDefaults. Each
  // must behave identically — the bug in issue #406 fired through all of them.
  const ENTRY_POINTS: Array<{
    label: string
    patch: Parameters<typeof patchCodexConfigTomlText>[1]
  }> = [
    { label: "API base URL", patch: { apiBaseUrl: "https://new.example/v1" } },
    { label: "WebSocket toggle", patch: { supportsWebsockets: true } },
    { label: "model provider", patch: { modelProvider: "codeg" } },
  ]

  const BOUND_PROVIDER = [
    'model_provider = "codeg"',
    "",
    "[model_providers.codeg]",
    'base_url = "https://old.example/v1"',
    'name = "codeg"',
    'wire_api = "responses"',
  ].join("\n")

  for (const { label, patch } of ENTRY_POINTS) {
    describe(`via the ${label} control`, () => {
      it("keeps an explicit false", () => {
        const toml = `${BOUND_PROVIDER}\nrequires_openai_auth = false\n`
        expect(authFlagOf(patchCodexConfigTomlText(toml, patch))).toBe(false)
      })

      it("keeps an explicit true", () => {
        const toml = `${BOUND_PROVIDER}\nrequires_openai_auth = true\n`
        expect(authFlagOf(patchCodexConfigTomlText(toml, patch))).toBe(true)
      })

      it("supplies the default when the field is absent", () => {
        expect(
          authFlagOf(patchCodexConfigTomlText(BOUND_PROVIDER, patch))
        ).toBe(true)
      })

      it("seeds a brand-new provider from an empty config", () => {
        expect(authFlagOf(patchCodexConfigTomlText("", patch))).toBe(true)
      })

      it("stands down for a provider using actor authorization", () => {
        const toml = [
          BOUND_PROVIDER,
          "",
          "[model_providers.codeg.http_headers]",
          'x-openai-actor-authorization = "local-image-extension"',
        ].join("\n")
        expect(
          authFlagOf(patchCodexConfigTomlText(toml, patch))
        ).toBeUndefined()
      })
    })
  }

  const ENTRY = ENTRY_POINTS[0].patch

  it("preserves user comments around the managed provider", () => {
    const toml = [
      "# my hand-written codex config",
      'model_provider = "codeg"',
      "",
      "[model_providers.codeg]",
      'base_url = "https://old.example/v1"',
      "# keep the actor-authorization arrangement intact",
      "requires_openai_auth = false",
    ].join("\n")
    const result = patchCodexConfigTomlText(toml, ENTRY)
    expect(result).toContain("# my hand-written codex config")
    expect(result).toContain(
      "# keep the actor-authorization arrangement intact"
    )
    expect(authFlagOf(result)).toBe(false)
  })

  it("matches the actor-authorization header case-insensitively", () => {
    const toml = [
      BOUND_PROVIDER,
      "",
      "[model_providers.codeg.http_headers]",
      'X-OpenAI-Actor-Authorization = "local-image-extension"',
    ].join("\n")
    expect(authFlagOf(patchCodexConfigTomlText(toml, ENTRY))).toBeUndefined()
  })

  it("reads the header out of an inline table too", () => {
    const toml = [
      'model_provider = "codeg"',
      "",
      "[model_providers.codeg]",
      'base_url = "https://old.example/v1"',
      'http_headers = { "x-openai-actor-authorization" = "local-image-extension" }',
    ].join("\n")
    expect(authFlagOf(patchCodexConfigTomlText(toml, ENTRY))).toBeUndefined()
  })

  // Asserted on the text, not the parse: the text writers predate this change
  // and cannot patch a provider spelled with root-level dotted keys — they
  // append a `[model_providers.codeg]` section that redefines the same table.
  // That limitation is pre-existing and out of scope here; what matters is
  // that the header is still recognized, so no auth default is added.
  it("reads the header out of a root-level dotted key too", () => {
    const toml = [
      'model_provider = "codeg"',
      'model_providers.codeg.base_url = "https://old.example/v1"',
      'model_providers.codeg.http_headers."x-openai-actor-authorization" = "local-image-extension"',
    ].join("\n")
    expect(patchCodexConfigTomlText(toml, ENTRY)).not.toContain(
      "requires_openai_auth"
    )
  })

  // Upstream's predicate is `!value.trim().is_empty()`, so a blank header does
  // NOT enable actor authorization and the default still applies.
  it("ignores an actor-authorization header with a blank value", () => {
    const toml = [
      BOUND_PROVIDER,
      "",
      "[model_providers.codeg.http_headers]",
      'x-openai-actor-authorization = "   "',
    ].join("\n")
    expect(authFlagOf(patchCodexConfigTomlText(toml, ENTRY))).toBe(true)
  })

  it("never rewrites an explicit true that sits beside the header", () => {
    const toml = [
      BOUND_PROVIDER,
      "requires_openai_auth = true",
      "",
      "[model_providers.codeg.http_headers]",
      'x-openai-actor-authorization = "local-image-extension"',
    ].join("\n")
    expect(authFlagOf(patchCodexConfigTomlText(toml, ENTRY))).toBe(true)
  })

  it("ignores another provider's actor-authorization header", () => {
    const toml = [
      BOUND_PROVIDER,
      "",
      "[model_providers.other.http_headers]",
      'x-openai-actor-authorization = "local-image-extension"',
    ].join("\n")
    expect(authFlagOf(patchCodexConfigTomlText(toml, ENTRY))).toBe(true)
  })

  // Regression (review round 1): `"http_headers.x-..." = "v"` is ONE literal
  // key, not an http_headers sub-table. Mistaking it for the nested path would
  // suppress the default and break codeg's own auth.json-based auth.
  it("does not mistake a quoted dotted key for the header table", () => {
    const toml = [
      BOUND_PROVIDER,
      '"http_headers.x-openai-actor-authorization" = "local-image-extension"',
    ].join("\n")
    expect(authFlagOf(patchCodexConfigTomlText(toml, ENTRY))).toBe(true)
  })

  // Regression (review round 2): an escaped key still declares the field, so
  // writing the default would produce a duplicate-key TOML the backend rejects.
  it("recognizes a field declared through an escaped quoted key", () => {
    const toml = [BOUND_PROVIDER, '"requires\\u005fopenai_auth" = false'].join(
      "\n"
    )
    const result = patchCodexConfigTomlText(toml, ENTRY)
    expect(authFlagOf(result)).toBe(false)
    expect(() => parseTomlDocument(result)).not.toThrow()
  })

  // A draft can be mid-edit in the raw editor. We cannot tell what is declared,
  // so we touch nothing — the backend refuses to persist invalid TOML anyway.
  it("leaves an unparsable draft's auth field alone", () => {
    const toml = [BOUND_PROVIDER, 'base_url = "unterminated'].join("\n")
    const result = patchCodexConfigTomlText(toml, ENTRY)
    expect(result).not.toContain("requires_openai_auth")
  })
})

describe("host-tools toggle — hand the fs/terminal channels back to the agent", () => {
  const KEY = "CODEG_ACP_HOST_TOOLS"

  it("is off for an agent that has never touched the knob", () => {
    expect(hostToolsAgentModeEnabled("")).toBe(false)
    expect(hostToolsAgentModeEnabled("XAI_API_KEY=abc")).toBe(false)
  })

  it("round-trips on and back off", () => {
    const on = setHostToolsAgentMode("XAI_API_KEY=abc", true)
    expect(on).toContain(`${KEY}=agent`)
    expect(hostToolsAgentModeEnabled(on)).toBe(true)

    // Off writes an EXPLICIT `default` rather than deleting the key. Deleting
    // would let a process-wide `CODEG_ACP_HOST_TOOLS=agent` keep winning, so
    // the switch could not turn the mode off at all — it would read false
    // while the next connection still withheld the channels.
    const off = setHostToolsAgentMode(on, false)
    expect(off).toContain(`${KEY}=default`)
    expect(hostToolsAgentModeEnabled(off)).toBe(false)
    expect(off).toContain("XAI_API_KEY=abc")
  })

  it("leaves the agent's other env vars alone", () => {
    const before = "XAI_API_KEY=abc\nGROK_HOME=/tmp/grok"
    const after = setHostToolsAgentMode(before, true)
    expect(after).toContain("XAI_API_KEY=abc")
    expect(after).toContain("GROK_HOME=/tmp/grok")
  })

  it("reads only the exact sentinel, matching the Rust resolver", () => {
    // The backend fails OPEN on an unrecognized value (a typo must look like a
    // typo, not like a silently disabled terminal), so the switch must render
    // unchecked for anything that is not literally `agent`.
    expect(hostToolsAgentModeEnabled(`${KEY}=default`)).toBe(false)
    expect(hostToolsAgentModeEnabled(`${KEY}=Agent`)).toBe(false)
    expect(hostToolsAgentModeEnabled(`${KEY}=off`)).toBe(false)
    expect(hostToolsAgentModeEnabled(`${KEY}=`)).toBe(false)
    // Trimmed, though — `parseEnvText` trims, and so does the backend.
    expect(hostToolsAgentModeEnabled(`${KEY} = agent `)).toBe(true)
  })

  it("does not double up when toggled on twice", () => {
    const once = setHostToolsAgentMode("", true)
    const twice = setHostToolsAgentMode(once, true)
    expect(twice).toBe(once)
    expect(twice.match(new RegExp(KEY, "g"))).toHaveLength(1)
  })

  it("overrides an inherited value in both directions", () => {
    // The backend resolves env_json first, then codeg's process env. Whatever
    // the operator exported, one flip of the switch must decide the outcome —
    // so both states write an explicit value and neither leaves the key absent.
    const off = setHostToolsAgentMode("", false)
    expect(off).toBe(`${KEY}=default`)
    expect(hostToolsAgentModeEnabled(off)).toBe(false)
    expect(hostToolsAgentModeEnabled(setHostToolsAgentMode(off, true))).toBe(
      true
    )
  })
})

describe("rebaseDeepSeekDraft", () => {
  // Only the fields this helper reads or writes; the rest of AgentDraft is
  // spread through untouched, which the identity assertion below pins.
  const draftOf = (envText: string) =>
    ({
      envText,
      apiBaseUrl: "",
      apiKey: "",
      model: "",
      enabled: true,
      configText: "",
    }) as unknown as Parameters<typeof rebaseDeepSeekDraft>[0]

  it("folds the panel's keys in while leaving every other key alone", () => {
    // The window's draft still holds the key it saw before another window
    // saved a new one; the enable switch would persist this text wholesale.
    //
    // `DEEPSEEK_ACP_MODEL` is NOT a panel key — the panel shows no model field
    // and never writes one, so the raw editor's line stays exactly as typed
    // even when the persisted env disagrees with it.
    const draft = draftOf(
      "DEEPSEEK_API_KEY=sk-old\nDEEPSEEK_ACP_MODEL=deepseek-v4-flash\nSOMETHING_ELSE=keep-me"
    )
    const agent = makeAgent({
      agent_type: "deepseek" as AgentType,
      env: {
        DEEPSEEK_API_KEY: "sk-new",
        DEEPSEEK_ACP_MODEL: "deepseek-v4-pro",
      },
    })

    const next = rebaseDeepSeekDraft(draft, agent)
    const lines = next.envText.split("\n").sort()
    expect(lines).toEqual([
      "DEEPSEEK_ACP_MODEL=deepseek-v4-flash",
      "DEEPSEEK_API_KEY=sk-new",
      "SOMETHING_ELSE=keep-me",
    ])
    // The structured mirrors are recomputed from the patched text.
    expect(next.apiKey).toBe("sk-new")
    expect(next.model).toBe("deepseek-v4-flash")
  })

  it("does not rebase for a model-only change", () => {
    // The model is not a panel key, so a model that moved elsewhere is not a
    // reason to rewrite the draft (and risk a half-typed line in it).
    const draft = draftOf("DEEPSEEK_ACP_MODEL=deepseek-v4-flash\nKEEP=1")
    const agent = makeAgent({
      agent_type: "deepseek" as AgentType,
      env: { DEEPSEEK_ACP_MODEL: "deepseek-v4-pro" },
    })
    expect(rebaseDeepSeekDraft(draft, agent)).toBe(draft)
  })

  it("deletes a panel key the persisted env no longer has", () => {
    const draft = draftOf("DEEPSEEK_BASE_URL=https://old.example.com\nKEEP=1")
    const agent = makeAgent({ agent_type: "deepseek" as AgentType, env: {} })
    expect(rebaseDeepSeekDraft(draft, agent).envText).toBe("KEEP=1")
  })

  it("returns the SAME object when nothing moved", () => {
    // A no-op refresh must not invalidate the draft identity.
    const draft = draftOf("DEEPSEEK_API_KEY=sk-1\nOTHER=2")
    const agent = makeAgent({
      agent_type: "deepseek" as AgentType,
      env: { DEEPSEEK_API_KEY: "sk-1", OTHER: "ignored" },
    })
    expect(rebaseDeepSeekDraft(draft, agent)).toBe(draft)
  })

  it("keeps comments and half-typed lines when a key DID move", () => {
    // The patch is textual: only the lines it owns are rewritten. A parse-and-
    // reserialize would drop all three of these, and this rebase runs on the
    // refresh that follows the panel's own save — i.e. while the user may well
    // be typing in the raw editor next to it.
    const draft = draftOf(
      "# proxy for the office network\nDEEPSEEK_API_KEY=sk-old\n\nNEW_PROXY\nKEEP=1"
    )
    const agent = makeAgent({
      agent_type: "deepseek" as AgentType,
      env: { DEEPSEEK_API_KEY: "sk-new" },
    })
    expect(rebaseDeepSeekDraft(draft, agent).envText).toBe(
      "# proxy for the office network\nDEEPSEEK_API_KEY=sk-new\n\nNEW_PROXY\nKEEP=1"
    )
  })

  it("survives an env var named after an Object.prototype member", () => {
    // `constructor`, `toString` and friends are legal env var names, and a
    // plain object answers `in` for all of them — reading one back yields a
    // function where a string was expected and throws mid-patch, after the
    // backend has already committed the save this rebase follows.
    const draft = draftOf(
      "constructor=keep\ntoString=keep\nDEEPSEEK_API_KEY=old"
    )
    const agent = makeAgent({
      agent_type: "deepseek" as AgentType,
      env: { DEEPSEEK_API_KEY: "sk-new" },
    })
    expect(rebaseDeepSeekDraft(draft, agent).envText).toBe(
      "constructor=keep\ntoString=keep\nDEEPSEEK_API_KEY=sk-new"
    )
  })

  it("appends a new key above a trailing blank line", () => {
    // That blank line is usually the one the user just pressed Enter on.
    const draft = draftOf("KEEP=1\n")
    const agent = makeAgent({
      agent_type: "deepseek" as AgentType,
      env: { DEEPSEEK_API_KEY: "sk-1" },
    })
    expect(rebaseDeepSeekDraft(draft, agent).envText).toBe(
      "KEEP=1\nDEEPSEEK_API_KEY=sk-1\n"
    )
  })

  it("treats a present-but-empty key as different from a missing one", () => {
    // `patchEnvText` deletes a key whose persisted value is empty, so `KEY=`
    // left in the draft IS a difference. The enable switch persists the draft
    // wholesale, and an empty DEEPSEEK_BASE_URL is not "use the default" — it
    // is an empty endpoint, which the adapter turns into an unusable URL.
    const draft = draftOf("DEEPSEEK_BASE_URL=\nKEEP=1")
    const agent = makeAgent({ agent_type: "deepseek" as AgentType, env: {} })
    expect(rebaseDeepSeekDraft(draft, agent).envText).toBe("KEEP=1")
  })

  it("leaves a draft mid-edit alone when no panel key moved", () => {
    // The rebase runs on every `refreshAgents`, including the one after an
    // unrelated save. Folding the keys in reparses and reserializes the whole
    // text, which drops comments, blank lines and a half-typed key — so the
    // decision has to be made on the VALUES, before any rewrite.
    const draft = draftOf(
      "# proxy for the office network\nDEEPSEEK_API_KEY=sk-1\n\nNEW_PROXY"
    )
    const agent = makeAgent({
      agent_type: "deepseek" as AgentType,
      env: { DEEPSEEK_API_KEY: "sk-1" },
    })
    expect(rebaseDeepSeekDraft(draft, agent)).toBe(draft)
  })
})

describe("important env keys", () => {
  // Qoder is the only agent with an EMPTY slot (it talks solely to its own
  // service, so there is no endpoint variable). `keys.apiBaseUrl[0]` is then
  // `undefined`, and the writer used to hand that straight to `patchEnvText` —
  // producing an env line literally named `undefined` and silently swallowing
  // the value the user typed.
  it("never writes an `undefined` key for a slot the agent has no var for", () => {
    expect(importantEnvKeysByAgent("qoder" as AgentType).apiBaseUrl).toEqual([])
    const before = "QODER_MODEL=qmodel_38max"
    const after = patchEnvByImportantKey(
      "qoder" as AgentType,
      before,
      "apiBaseUrl",
      "https://example.com"
    )
    expect(after).toBe(before)
    expect(after).not.toContain("undefined")
  })

  // …and the field is hidden, so the no-op above is unreachable in practice
  // rather than a silently-ignored input the user can type into.
  it("hides only the fields whose slot is empty", () => {
    expect(importantFieldsFor("qoder" as AgentType)).toEqual({
      apiBaseUrl: false,
      apiKey: true,
      model: true,
    })
    expect(importantFieldsFor("deepseek" as AgentType)).toEqual({
      apiBaseUrl: true,
      apiKey: true,
      model: true,
    })
    // The generic fallback keeps all three for agents with no special-casing.
    expect(importantFieldsFor("open_code" as AgentType)).toEqual({
      apiBaseUrl: true,
      apiKey: true,
      model: true,
    })
  })

  // The credential Qoder actually reads. Generic OPENAI_*/API_KEY aliases must
  // stay out: Qoder reads neither, so listing them would let the auth panel
  // report "configured" off a key that never reaches the agent.
  it("writes Qoder's PAT and model to the vars the CLI documents", () => {
    const keys = importantEnvKeysByAgent("qoder" as AgentType)
    expect(keys.apiKey).toEqual(["QODER_PERSONAL_ACCESS_TOKEN"])
    expect(keys.model).toEqual(["QODER_MODEL"])

    const withPat = patchEnvByImportantKey(
      "qoder" as AgentType,
      "",
      "apiKey",
      "pat-123"
    )
    expect(withPat).toBe("QODER_PERSONAL_ACCESS_TOKEN=pat-123")
    expect(
      patchEnvByImportantKey(
        "qoder" as AgentType,
        withPat,
        "model",
        "qmodel_38max"
      )
    ).toContain("QODER_MODEL=qmodel_38max")
  })
})
