import { render, screen, act, fireEvent } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"

const call = vi.fn()
// Capture the provider's app_update_state handler so tests can push live
// lifecycle transitions.
let liveHandler: ((s: unknown) => void) | null = null
const subscribe = vi.fn(
  async (_event: string, handler: (s: unknown) => void) => {
    liveHandler = handler
    return () => {}
  }
)

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call, subscribe }),
  isDesktop: () => false,
  isRemoteDesktopMode: () => false,
  getActiveRemoteConnectionId: () => null,
}))

vi.mock("@/lib/api", () => ({
  getSystemProxySettings: vi.fn(),
  updateSystemProxySettings: vi.fn(),
  updateSystemLanguageSettings: vi.fn(),
  // The System page now embeds <BackupSettings/>, which subscribes to backup
  // progress on mount and imports the backup API surface; stub it all.
  listenBackupProgress: vi.fn(async () => () => {}),
  exportBackupDesktop: vi.fn(),
  exportBackupWeb: vi.fn(),
  inspectBackupDesktop: vi.fn(),
  inspectBackupWeb: vi.fn(),
  scanExternalConflictsDesktop: vi.fn(),
  scanExternalConflictsWeb: vi.fn(),
  stageRestoreDesktop: vi.fn(),
  stageRestoreWeb: vi.fn(),
  uploadBackupWeb: vi.fn(),
}))

vi.mock("sonner", () => ({
  toast: { success: vi.fn(), error: vi.fn(), message: vi.fn() },
}))

vi.mock("@/lib/platform", () => ({ openUrl: vi.fn(), isDesktop: () => false }))

vi.mock("@/components/i18n-provider", () => ({
  useAppI18n: () => ({
    languageSettings: { mode: "system", language: "en" },
    languageSettingsLoaded: true,
    setLanguageSettings: vi.fn(),
  }),
}))

// Keep the test hermetic from the markdown ESM stack (only rendered when an
// update is available, which it isn't here).
vi.mock("react-markdown", () => ({
  default: ({ children }: { children?: string }) => children ?? null,
}))
vi.mock("remark-gfm", () => ({ default: () => undefined }))

import { SystemNetworkSettings } from "./system-network-settings"
import { UpdateProvider } from "@/components/providers/update-provider"
import enMessages from "@/i18n/messages/en.json"
import { getSystemProxySettings } from "@/lib/api"

const mockGetProxy = vi.mocked(getSystemProxySettings)

// The settings page reads the update lifecycle from the app-wide UpdateProvider
// (settings/layout.tsx wraps it in production), so the test must too.
function renderWithIntl() {
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <UpdateProvider>
        <SystemNetworkSettings />
      </UpdateProvider>
    </NextIntlClientProvider>
  )
}

beforeEach(() => {
  call.mockReset()
  subscribe.mockClear()
  mockGetProxy.mockReset()
  liveHandler = null
})

// A new server reporting an available update + rollback + the live-progress
// protocol, parameterized by the app_update_state snapshot the provider sees.
function liveServerCalls(snapshot: unknown) {
  return async (endpoint: string) => {
    if (endpoint === "check_app_update") {
      return {
        currentVersion: "0.16.0",
        update: null,
        selfUpdateSupported: true,
        capability: "supervised",
        runtime: "standalone",
        restartDelayMs: 2000,
        rollbackAvailable: true,
        liveProgress: true,
      }
    }
    if (endpoint === "app_update_status") {
      return {
        currentVersion: "0.16.0",
        selfUpdateSupported: true,
        capability: "supervised",
        runtime: "standalone",
        restartDelayMs: 2000,
        rollbackAvailable: true,
        liveProgress: true,
      }
    }
    if (endpoint === "health") return { version: "0.16.0" }
    if (endpoint === "app_update_state") return snapshot
    throw new Error(`unexpected endpoint: ${endpoint}`)
  }
}

describe("SystemNetworkSettings — update source outage", () => {
  it("loads proxy settings and exposes rollback when the manifest is unreachable", async () => {
    // The release source is down: the update CHECK fails, but the version read
    // and rollback availability come from the local `app_update_status`
    // endpoint, so neither the settings load nor the rollback action breaks.
    mockGetProxy.mockResolvedValue({
      enabled: true,
      proxy_url: "http://proxy.local:8080",
    })
    call.mockImplementation(async (endpoint: string) => {
      if (endpoint === "check_app_update") {
        throw new Error("manifest unreachable")
      }
      if (endpoint === "app_update_status") {
        return {
          currentVersion: "0.14.11",
          selfUpdateSupported: true,
          capability: "supervised",
          runtime: "standalone",
          restartDelayMs: 2000,
          rollbackAvailable: true,
        }
      }
      if (endpoint === "health") return { version: "0.14.11" }
      if (endpoint === "app_update_state") return { seq: 0, status: "idle" }
      throw new Error(`unexpected endpoint: ${endpoint}`)
    })

    renderWithIntl()

    // Rollback action is exposed despite the failed update check.
    expect(
      await screen.findByRole("button", { name: "Roll back" })
    ).toBeInTheDocument()

    // Unrelated local settings still loaded (not defaulted), and the settings
    // load itself did not error out.
    expect(
      screen.getByDisplayValue("http://proxy.local:8080")
    ).toBeInTheDocument()
    expect(screen.queryByText(/Load failed/)).not.toBeInTheDocument()
  })

  it("loads proxy settings even when the status route is also unavailable (older server)", async () => {
    // Newer desktop, older remote server: both the update check and the new
    // /app_update_status route fail; the version still resolves via /health and
    // the settings load must not break.
    mockGetProxy.mockResolvedValue({
      enabled: true,
      proxy_url: "http://proxy.local:8080",
    })
    call.mockImplementation(async (endpoint: string) => {
      if (endpoint === "check_app_update") {
        throw new Error("manifest unreachable")
      }
      if (endpoint === "app_update_status") {
        throw new Error("not implemented")
      }
      if (endpoint === "health") return { version: "0.14.11" }
      if (endpoint === "app_update_state") return { seq: 0, status: "idle" }
      throw new Error(`unexpected endpoint: ${endpoint}`)
    })

    renderWithIntl()

    // Settings load completed (spinner gone) and proxy is loaded, not defaulted.
    expect(
      await screen.findByDisplayValue("http://proxy.local:8080")
    ).toBeInTheDocument()
    expect(screen.queryByText(/Load failed/)).not.toBeInTheDocument()
  })

  it("falls back to 'view release' for a legacy server lacking live_progress", async () => {
    // Newer client, older remote server: it reports an available update and
    // self-update capability, but NOT the `liveProgress` protocol flag. The
    // client must not drive the new detached flow against its old blocking
    // endpoint — it shows the release link instead and never calls perform.
    mockGetProxy.mockResolvedValue({ enabled: false, proxy_url: null })
    call.mockImplementation(async (endpoint: string) => {
      if (endpoint === "check_app_update") {
        return {
          currentVersion: "0.14.0",
          update: { version: "0.16.0", body: "", date: null },
          selfUpdateSupported: true,
          capability: "supervised",
          runtime: "standalone",
          restartDelayMs: 2000,
          rollbackAvailable: false,
          // liveProgress intentionally absent (older server).
        }
      }
      if (endpoint === "app_update_status") {
        return {
          currentVersion: "0.14.0",
          selfUpdateSupported: true,
          capability: "supervised",
          runtime: "standalone",
          restartDelayMs: 2000,
          rollbackAvailable: false,
        }
      }
      if (endpoint === "health") return { version: "0.14.0" }
      if (endpoint === "app_update_state") return { seq: 0, status: "idle" }
      throw new Error(`unexpected endpoint: ${endpoint}`)
    })

    renderWithIntl()

    // The release-link affordance is shown, not the in-place upgrade button.
    expect(
      await screen.findByRole("button", { name: "View v0.16.0 release" })
    ).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Upgrade to v0.16.0" })
    ).not.toBeInTheDocument()
    // The new detached flow was never started against the legacy server.
    expect(call).not.toHaveBeenCalledWith(
      "perform_app_update",
      expect.anything(),
      expect.anything()
    )
  })

  it("suppresses rollback while an upgrade is staged (ready_to_restart)", async () => {
    // A downloaded upgrade is staged AND a previous version is rollbackable.
    // Offering both "Restart to update" and "Roll back" would conflict (and let
    // another window restart into the staged build mid-rollback), so rollback
    // must be hidden until the lifecycle returns to idle.
    mockGetProxy.mockResolvedValue({ enabled: false, proxy_url: null })
    call.mockImplementation(
      liveServerCalls({
        seq: 5,
        status: "ready_to_restart",
        version: "0.17.0",
        restartDelayMs: 2000,
        trialSeconds: 30,
        capability: "supervised",
      })
    )

    renderWithIntl()

    // The staged-update restart prompt is shown…
    expect(
      await screen.findByRole("button", { name: "Restart to update" })
    ).toBeInTheDocument()
    // …and the conflicting rollback action is suppressed despite being
    // available.
    expect(
      screen.queryByRole("button", { name: "Roll back" })
    ).not.toBeInTheDocument()
  })

  it("closes a stale rollback dialog when an upgrade becomes staged", async () => {
    // Idle to begin with, so rollback is offered and its confirm dialog opens.
    mockGetProxy.mockResolvedValue({ enabled: false, proxy_url: null })
    call.mockImplementation(liveServerCalls({ seq: 1, status: "idle" }))

    renderWithIntl()

    const rollbackBtn = await screen.findByRole("button", { name: "Roll back" })
    fireEvent.click(rollbackBtn)
    expect(
      await screen.findByText("Roll back to the previous version?")
    ).toBeInTheDocument()

    // Another window stages an update: the shared state advances to
    // ready_to_restart. The now-conflicting dialog must close on its own.
    await act(async () => {
      liveHandler?.({ seq: 9, status: "ready_to_restart", version: "0.17.0" })
      await Promise.resolve()
    })
    expect(
      screen.queryByText("Roll back to the previous version?")
    ).not.toBeInTheDocument()
  })

  it("does not flicker rollback before the live-progress snapshot hydrates", async () => {
    // A live-progress server reports rollback availability immediately, but the
    // authoritative app_update_state snapshot is slow. Rollback must stay hidden
    // until it hydrates (the provider's default `idle` is only a placeholder),
    // then appear if the real state is idle.
    let resolveSnapshot!: (s: unknown) => void
    const pending = new Promise<unknown>((r) => {
      resolveSnapshot = r
    })
    mockGetProxy.mockResolvedValue({ enabled: false, proxy_url: null })
    call.mockImplementation(liveServerCalls(pending))

    renderWithIntl()

    // Status/check have resolved (rollbackAvailable + liveProgress true), but
    // the snapshot is still in flight — rollback must NOT show yet.
    expect(
      await screen.findByRole("button", { name: "Check for updates" })
    ).toBeInTheDocument()
    expect(
      screen.queryByRole("button", { name: "Roll back" })
    ).not.toBeInTheDocument()

    // The snapshot hydrates to a real idle state — rollback may now appear.
    await act(async () => {
      resolveSnapshot({ seq: 1, status: "idle" })
      await Promise.resolve()
    })
    expect(
      await screen.findByRole("button", { name: "Roll back" })
    ).toBeInTheDocument()
  })
})
