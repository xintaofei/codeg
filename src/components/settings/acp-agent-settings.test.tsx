import { describe, expect, it } from "vitest"

import { buildVersionCheck, getAgentChecks } from "./acp-agent-settings"
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
    base_cli_version: null,
    base_cli_command: null,
    base_cli_package: null,
    env: {},
    config_json: null,
    config_file_path: null,
    opencode_auth_json: null,
    codex_auth_json: null,
    cline_secrets_json: null,
    codex_config_toml: null,
    hermes_config_yaml: null,
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

  it("warns instead of failing when only the upstream CLI is detected", () => {
    const check = buildVersionCheck(
      makeAgent({
        agent_type: "codex" as AgentType,
        distribution_type: "binary",
        installed_version: null,
        base_cli_version: "0.128.0",
        base_cli_command: "codex",
        base_cli_package: "@openai/codex",
      })
    )

    expect(check?.status).toBe("warn")
    expect(check?.message).toContain("codex 0.128.0")
    expect(check?.fixes.some((fix) => fix.kind === "download_binary")).toBe(
      true
    )
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
  const systemCliPreflight: { result: PreflightResult } = {
    result: {
      agent_type: "hermes" as AgentType,
      agent_name: "Hermes Agent",
      passed: true,
      checks: [
        {
          check_id: "uv_available",
          label: "uv",
          status: "warn",
          message: "uv not found; will launch via the system `hermes` command",
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

  it("does not block when uv is missing but a system uvx-agent CLI is launchable", () => {
    const checks = getAgentChecks(
      makeAgent({
        distribution_type: "uvx",
        available: true,
        installed_version: "unknown",
      }),
      systemCliPreflight
    )

    const versionCheck = checks.find((c) => c.check_id === "version_status")
    expect(versionCheck?.status).toBe("warn")
    expect(versionCheck?.fixes.some((fix) => fixDisabled(fix))).toBe(false)
    expect(versionCheck?.message).toContain("unknown")
  })
})
