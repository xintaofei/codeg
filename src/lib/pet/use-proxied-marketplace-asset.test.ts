import { renderHook, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

vi.mock("./api", () => ({ fetchMarketplaceAsset: vi.fn() }))

// Track blob create/revoke without the real URL object API (jsdom lacks it and
// we want to assert exact lifecycle). Each create returns a unique url so we can
// prove consumers get *distinct* blobs off one shared asset.
let blobSeq = 0
const created: string[] = []
const revoked: string[] = []
vi.mock("./sprite-url", () => ({
  createPetSpriteObjectUrl: vi.fn((asset: { dataBase64: string }) => {
    const url = `blob:${asset.dataBase64}#${blobSeq++}`
    created.push(url)
    return url
  }),
  revokePetSpriteObjectUrl: vi.fn((url: string | null | undefined) => {
    if (url) revoked.push(url)
  }),
}))

import {
  __resetMarketplaceAssetCacheForTests,
  useProxiedMarketplaceAsset,
} from "./use-proxied-marketplace-asset"
import { fetchMarketplaceAsset } from "./api"
import type { PetSpriteAsset } from "./types"

const mockFetch = vi.mocked(fetchMarketplaceAsset)

function asset(data: string): PetSpriteAsset {
  return { mime: "image/webp", dataBase64: data }
}

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((res, rej) => {
    resolve = res
    reject = rej
  })
  return { promise, resolve, reject }
}

beforeEach(() => {
  blobSeq = 0
  created.length = 0
  revoked.length = 0
  mockFetch.mockReset()
  __resetMarketplaceAssetCacheForTests()
})

afterEach(() => {
  vi.clearAllMocks()
})

describe("useProxiedMarketplaceAsset", () => {
  it("resolves a url to a blob and revokes exactly that blob on unmount", async () => {
    mockFetch.mockResolvedValue(asset("AAA"))

    const { result, unmount } = renderHook(() =>
      useProxiedMarketplaceAsset("https://codex-pets.net/a/poster.webp")
    )
    expect(result.current).toEqual({ src: null, loading: true, failed: false })

    await waitFor(() => expect(result.current.src).toBeTruthy())
    const src = result.current.src as string
    expect(result.current.failed).toBe(false)
    expect(created).toContain(src)
    expect(revoked).not.toContain(src)

    unmount()
    expect(revoked).toEqual([src])
  })

  // Guards Codex blocking issue #1: a shared cache entry must never be revoked
  // out from under a still-mounted consumer. Each consumer owns its own blob.
  it("gives each consumer its own blob so unmounting one never blanks another", async () => {
    mockFetch.mockResolvedValue(asset("SHARED"))
    const url = "https://codex-pets.net/shared/poster.webp"

    const a = renderHook(() => useProxiedMarketplaceAsset(url))
    const b = renderHook(() => useProxiedMarketplaceAsset(url))

    await waitFor(() => expect(a.result.current.src).toBeTruthy())
    await waitFor(() => expect(b.result.current.src).toBeTruthy())

    const aSrc = a.result.current.src as string
    const bSrc = b.result.current.src as string
    expect(aSrc).not.toBe(bSrc) // distinct per-consumer blobs...
    expect(mockFetch).toHaveBeenCalledTimes(1) // ...off a single shared fetch

    a.unmount()
    expect(revoked).toContain(aSrc)
    expect(revoked).not.toContain(bSrc) // b's image stays live

    b.unmount()
    expect(revoked).toContain(bSrc)
  })

  // Guards Codex blocking issue #2: a rejection must not be cached (retryable),
  // and a failed fetch surfaces `failed` without ever minting a blob.
  it("does not cache a rejection and reports failure", async () => {
    mockFetch.mockRejectedValueOnce(new Error("network down"))
    mockFetch.mockResolvedValueOnce(asset("RECOVERED"))
    const url = "https://codex-pets.net/flaky/poster.webp"

    const first = renderHook(() => useProxiedMarketplaceAsset(url))
    await waitFor(() => expect(first.result.current.failed).toBe(true))
    expect(first.result.current.src).toBeNull()
    expect(created).toHaveLength(0) // no orphan blob on failure
    first.unmount()

    // A fresh mount re-fetches (rejection was evicted, not cached) and succeeds.
    const second = renderHook(() => useProxiedMarketplaceAsset(url))
    await waitFor(() => expect(second.result.current.src).toBeTruthy())
    expect(mockFetch).toHaveBeenCalledTimes(2)
  })

  it("does not mint a blob when unmounted before the fetch settles", async () => {
    const d = deferred<PetSpriteAsset>()
    mockFetch.mockReturnValue(d.promise)

    const { unmount } = renderHook(() =>
      useProxiedMarketplaceAsset("https://codex-pets.net/slow/poster.webp")
    )
    unmount() // cancel before settle
    d.resolve(asset("LATE"))
    await Promise.resolve()
    await Promise.resolve()

    expect(created).toHaveLength(0)
    expect(revoked).toHaveLength(0)
  })

  it("reports not-loading and mints nothing for a nullish url", () => {
    const { result } = renderHook(() => useProxiedMarketplaceAsset(null))
    expect(result.current).toEqual({ src: null, loading: false, failed: false })
    expect(mockFetch).not.toHaveBeenCalled()
  })
})
