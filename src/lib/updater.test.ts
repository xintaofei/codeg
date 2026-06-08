import { beforeEach, describe, expect, it, vi } from "vitest"

const call = vi.fn()

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call }),
  isDesktop: () => false,
}))

import {
  confirmRollbackVersion,
  getCurrentAppVersion,
  getRunningServerVersion,
  getServerUpdateStatus,
  readServerVersionStrict,
  waitForServerHealthy,
} from "@/lib/updater"

describe("readServerVersionStrict", () => {
  beforeEach(() => {
    call.mockReset()
  })

  it("returns the version when /health reports one", async () => {
    call.mockResolvedValueOnce({ version: "0.14.12" })
    await expect(readServerVersionStrict()).resolves.toBe("0.14.12")
  })

  it("resolves null when /health responds without a version (older server)", async () => {
    call.mockResolvedValueOnce({})
    await expect(readServerVersionStrict()).resolves.toBeNull()
  })

  it("rejects (does not swallow) when the server is unreachable", async () => {
    call.mockRejectedValueOnce(new Error("down"))
    await expect(readServerVersionStrict()).rejects.toThrow("down")
  })
})

describe("getRunningServerVersion", () => {
  beforeEach(() => {
    call.mockReset()
  })

  it("returns the version on success", async () => {
    call.mockResolvedValueOnce({ version: "1.2.3" })
    await expect(getRunningServerVersion()).resolves.toBe("1.2.3")
  })

  it("swallows a transport failure as null", async () => {
    call.mockRejectedValueOnce(new Error("down"))
    await expect(getRunningServerVersion()).resolves.toBeNull()
  })
})

describe("getServerUpdateStatus", () => {
  beforeEach(() => {
    call.mockReset()
  })

  it("reads local self-update status from app_update_status (no manifest fetch)", async () => {
    // The rollback affordance must survive an unreachable update source, so it
    // is driven by this local endpoint rather than the manifest-dependent
    // check. A failing check_app_update is irrelevant here — this never calls it.
    call.mockResolvedValueOnce({
      currentVersion: "0.14.11",
      selfUpdateSupported: true,
      capability: "supervised",
      runtime: "docker",
      restartDelayMs: 2000,
      rollbackAvailable: true,
    })

    const status = await getServerUpdateStatus()

    expect(call).toHaveBeenCalledWith("app_update_status")
    expect(status).toEqual({
      currentVersion: "0.14.11",
      selfUpdateSupported: true,
      capability: "supervised",
      runtime: "docker",
      restartDelayMs: 2000,
      rollbackAvailable: true,
    })
  })
})

describe("getCurrentAppVersion (server mode)", () => {
  beforeEach(() => {
    call.mockReset()
  })

  it("reads the version from app_update_status, never check_app_update", async () => {
    // Regression: the settings page loads the version alongside unrelated local
    // state, so the version read must not depend on the (network) update check.
    call.mockResolvedValueOnce({
      currentVersion: "0.14.11",
      selfUpdateSupported: true,
      capability: "reexec",
      runtime: "standalone",
      restartDelayMs: 2000,
      rollbackAvailable: false,
    })

    const version = await getCurrentAppVersion()

    expect(version).toBe("0.14.11")
    expect(call).toHaveBeenCalledTimes(1)
    expect(call).toHaveBeenCalledWith("app_update_status")
    expect(call).not.toHaveBeenCalledWith("check_app_update")
  })

  it("falls open to /health when app_update_status is unavailable, never check_app_update", async () => {
    // A newer desktop can talk to an older server lacking /app_update_status;
    // the version read must not throw (it loads alongside unrelated settings)
    // and must never reach the manifest-dependent check.
    call.mockRejectedValueOnce(new Error("not implemented")) // app_update_status
    call.mockResolvedValueOnce({ version: "0.14.11" }) // /health

    const version = await getCurrentAppVersion()

    expect(version).toBe("0.14.11")
    expect(call).toHaveBeenCalledWith("app_update_status")
    expect(call).toHaveBeenCalledWith("health", {}, { timeoutMs: 4000 })
    expect(call).not.toHaveBeenCalledWith("check_app_update")
  })

  it("resolves to 'unknown' when both the status route and /health fail", async () => {
    call.mockRejectedValueOnce(new Error("not implemented")) // app_update_status
    call.mockRejectedValueOnce(new Error("down")) // /health

    await expect(getCurrentAppVersion()).resolves.toBe("unknown")
    expect(call).not.toHaveBeenCalledWith("check_app_update")
  })
})

describe("confirmRollbackVersion", () => {
  beforeEach(() => {
    call.mockReset()
  })

  it("counts a healthy-but-versionless target as rolled-back (not a timeout)", async () => {
    // The rollback target can be an older build whose /health omits the
    // version. The server is up and the previous bundle is restored, so this
    // must NOT be reported as a failed/timed-out restart.
    call.mockResolvedValue({}) // /health answers, but with no version

    await expect(confirmRollbackVersion("0.15.0")).resolves.toBe("rolled-back")
    expect(call).toHaveBeenCalledWith("health", {}, { timeoutMs: 4000 })
  })

  it("counts a moved version as rolled-back", async () => {
    call.mockResolvedValue({ version: "0.14.0" })
    await expect(confirmRollbackVersion("0.15.0")).resolves.toBe("rolled-back")
  })

  it("reports unchanged when the version stays on the pre-rollback value", async () => {
    call.mockResolvedValue({ version: "0.15.0" })
    await expect(
      confirmRollbackVersion("0.15.0", { attempts: 2, intervalMs: 1 })
    ).resolves.toBe("unchanged")
  })

  it("reports unreachable when /health never answers", async () => {
    call.mockRejectedValue(new Error("down"))
    await expect(
      confirmRollbackVersion("0.15.0", { attempts: 2, intervalMs: 1 })
    ).resolves.toBe("unreachable")
  })
})

describe("waitForServerHealthy", () => {
  beforeEach(() => {
    call.mockReset()
  })

  it("resolves true as soon as /health answers", async () => {
    // First poll fails (server still restarting), second succeeds.
    call.mockRejectedValueOnce(new Error("down")).mockResolvedValueOnce({})

    const healthy = await waitForServerHealthy({
      timeoutMs: 5_000,
      intervalMs: 5,
    })

    expect(healthy).toBe(true)
    expect(call).toHaveBeenCalledWith("health", {}, { timeoutMs: 4000 })
    expect(call).toHaveBeenCalledTimes(2)
  })

  it("resolves false when the server never comes back before the deadline", async () => {
    call.mockRejectedValue(new Error("down"))

    const healthy = await waitForServerHealthy({
      timeoutMs: 30,
      intervalMs: 5,
    })

    expect(healthy).toBe(false)
    expect(call.mock.calls.length).toBeGreaterThan(0)
  })
})
