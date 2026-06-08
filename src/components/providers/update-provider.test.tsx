import { render, screen, act, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import type { AppUpdateState } from "@/lib/updater"

// The provider's snapshot/subscribe helpers go through getTransport(), so
// mocking the transport drives the whole provider without stubbing updater.ts.
let snapshot: AppUpdateState = { seq: 0, status: "idle" }
// Per-call overrides for the app_update_state fetch (e.g. a hanging promise so
// a test can interleave events/reconnects before it resolves). Falls back to
// `snapshot` when empty.
let callQueue: Array<AppUpdateState | Promise<AppUpdateState>> = []
let reconnectCb: (() => void) | null = null
let liveHandler: ((s: AppUpdateState) => void) | null = null

const call = vi.fn(async (endpoint: string) => {
  if (endpoint === "app_update_state") {
    return callQueue.length ? callQueue.shift()! : snapshot
  }
  throw new Error(`unexpected endpoint: ${endpoint}`)
})
const subscribe = vi.fn(
  async (_event: string, handler: (s: AppUpdateState) => void) => {
    liveHandler = handler
    return () => {}
  }
)
const onReconnect = vi.fn((cb: () => void) => {
  reconnectCb = cb
  return () => {}
})

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call, subscribe, onReconnect }),
  isDesktop: () => false,
  isRemoteDesktopMode: () => false,
}))

vi.mock("sonner", () => ({ toast: { success: vi.fn(), error: vi.fn() } }))

import { UpdateProvider, useAppUpdate } from "./update-provider"
import enMessages from "@/i18n/messages/en.json"

function Probe() {
  const u = useAppUpdate()
  return (
    <div data-testid="status">
      {u?.state.status} #{u?.state.seq}
    </div>
  )
}

const makeTree = () => (
  <NextIntlClientProvider locale="en" messages={enMessages}>
    <UpdateProvider>
      <Probe />
    </UpdateProvider>
  </NextIntlClientProvider>
)

const text = () => screen.getByTestId("status").textContent

beforeEach(() => {
  call.mockClear()
  subscribe.mockClear()
  onReconnect.mockClear()
  reconnectCb = null
  liveHandler = null
  callQueue = []
  snapshot = { seq: 0, status: "idle" }
})

describe("UpdateProvider", () => {
  it("seeds current state from the snapshot on mount", async () => {
    snapshot = { seq: 50, status: "downloading", downloaded: 10, total: 100 }
    render(makeTree())
    await waitFor(() => expect(text()).toBe("downloading #50"))
  })

  it("accepts a lower-seq snapshot after a reconnect (backend restarted)", async () => {
    snapshot = { seq: 50, status: "downloading", downloaded: 10, total: 100 }
    render(makeTree())
    await waitFor(() => expect(text()).toBe("downloading #50"))

    // The server self-updated and relaunched: the NEW process restarts its seq
    // counter at 0 and is idle. Without resetting the high-water on reconnect,
    // the seq guard would discard this and leave the UI stuck on "downloading".
    snapshot = { seq: 0, status: "idle" }
    await act(async () => {
      reconnectCb?.()
      await Promise.resolve()
    })
    await waitFor(() => expect(text()).toBe("idle #0"))
  })

  it("keeps a higher-seq live event that arrives during the reconnect fetch", async () => {
    snapshot = { seq: 50, status: "downloading", downloaded: 10, total: 100 }
    render(makeTree())
    await waitFor(() => expect(text()).toBe("downloading #50"))

    // Reconnect with a snapshot fetch that hangs, so we can interleave a live
    // event from the new process before it resolves.
    let resolveSnap!: (s: AppUpdateState) => void
    callQueue = [
      new Promise<AppUpdateState>((r) => {
        resolveSnap = r
      }),
    ]
    await act(async () => {
      reconnectCb?.()
      await Promise.resolve()
    })

    // New process delivers a fresh, higher-seq event while the snapshot is in
    // flight — it must be applied (the high-water was reset before the fetch).
    await act(async () => {
      liveHandler?.({ seq: 3, status: "installing" })
      await Promise.resolve()
    })
    await waitFor(() => expect(text()).toBe("installing #3"))

    // The in-flight snapshot now resolves with an OLDER seq — it must NOT
    // clobber the newer live event.
    await act(async () => {
      resolveSnap({ seq: 1, status: "downloading", downloaded: 5, total: 100 })
      await Promise.resolve()
    })
    expect(text()).toBe("installing #3")
  })

  it("discards a pre-reconnect snapshot that resolves after the reset", async () => {
    // The mount snapshot fetch hangs; a reconnect happens before it resolves.
    let resolveMount!: (s: AppUpdateState) => void
    callQueue = [
      new Promise<AppUpdateState>((r) => {
        resolveMount = r
      }),
    ]
    render(makeTree())
    // Let the mount resync start and claim the hanging fetch (epoch 1) before
    // the reconnect, so the hang belongs to the pre-reconnect fetch.
    await act(async () => {
      await Promise.resolve()
      await Promise.resolve()
    })

    // Reconnect to the new (idle) process while the mount fetch is still in
    // flight.
    snapshot = { seq: 0, status: "idle" }
    await act(async () => {
      reconnectCb?.()
      await Promise.resolve()
    })
    await waitFor(() => expect(text()).toBe("idle #0"))

    // The original mount fetch finally resolves with the OLD process's high
    // seq — the epoch guard must discard it rather than resurrect the old
    // "downloading" state.
    await act(async () => {
      resolveMount({ seq: 50, status: "downloading", downloaded: 1, total: 2 })
      await Promise.resolve()
    })
    expect(text()).toBe("idle #0")
  })

  it("ignores a malformed payload (legacy server) without poisoning the guard", async () => {
    snapshot = { seq: 50, status: "downloading", downloaded: 10, total: 100 }
    render(makeTree())
    await waitFor(() => expect(text()).toBe("downloading #50"))

    // An older remote server's perform answers with the legacy
    // `{ version, needRestart }` shape (no seq/status) — it must be ignored.
    await act(async () => {
      liveHandler?.({
        version: "9.9.9",
        needRestart: true,
      } as unknown as AppUpdateState)
      await Promise.resolve()
    })
    expect(text()).toBe("downloading #50")

    // A subsequent valid event still applies — the guard wasn't poisoned by an
    // undefined seq.
    await act(async () => {
      liveHandler?.({ seq: 51, status: "installing" })
      await Promise.resolve()
    })
    await waitFor(() => expect(text()).toBe("installing #51"))
  })
})
