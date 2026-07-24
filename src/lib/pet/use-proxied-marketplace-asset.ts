import { useEffect, useState } from "react"
import { fetchMarketplaceAsset } from "./api"
import {
  createPetSpriteObjectUrl,
  revokePetSpriteObjectUrl,
} from "./sprite-url"
import type { PetSpriteAsset } from "./types"

// Cache the fetched *asset data* per URL (immutable, content-addressed `/v/…`
// paths) — NOT the blob URL. Paging back / reopening the dialog then resolves
// without another codex-pets fetch (which is the whole reason for the proxy:
// that host is slow/unreachable for these users). Each consumer mints its own
// blob URL from the shared asset and revokes it on unmount, so blob lifetimes
// are strictly per-consumer: a shared entry can never be revoked out from under
// a still-mounted consumer, and closing the dialog can't strand a live image.
const assetCache = new Map<string, Promise<PetSpriteAsset>>()

function loadAsset(url: string): Promise<PetSpriteAsset> {
  const existing = assetCache.get(url)
  if (existing) return existing

  const promise = fetchMarketplaceAsset(url)
  assetCache.set(url, promise)
  // Don't cache a rejection — a transient network blip should be retryable when
  // a component next mounts. Attached separately (not via the cached promise's
  // own chain) so consumers still observe the original rejection, and guarded on
  // identity so a superseded request's late failure can't evict a newer entry.
  promise.catch(() => {
    if (assetCache.get(url) === promise) assetCache.delete(url)
  })
  return promise
}

export interface ProxiedAsset {
  /** Blob URL for the proxied image, or `null` while loading / on failure. */
  src: string | null
  loading: boolean
  failed: boolean
}

interface Outcome {
  src: string | null
  failed: boolean
}

/**
 * Resolve a codex-pets.net image URL to a locally-served blob URL by proxying
 * the bytes through the backend (`pet_marketplace_asset`). The desktop webview
 * can't reach the CDN directly on some networks; the backend can, so this keeps
 * marketplace posters / previews rendering wherever the listing loads.
 *
 * Keyed by URL: a render where `url` just changed reports `loading` until the new
 * asset resolves rather than briefly showing the previous pet's image (mirrors
 * `useImageNaturalSize`). State is only ever written from the async callbacks,
 * never synchronously in the effect body. The blob URL is minted per consumer
 * and revoked in cleanup, so it is owned exclusively by this hook instance.
 */
export function useProxiedMarketplaceAsset(
  url: string | null | undefined
): ProxiedAsset {
  const [state, setState] = useState<{ url: string | null; outcome: Outcome }>(
    () => ({ url: null, outcome: { src: null, failed: false } })
  )

  useEffect(() => {
    if (!url) return

    let cancelled = false
    let objectUrl: string | null = null
    loadAsset(url)
      .then((asset) => {
        if (cancelled) return
        objectUrl = createPetSpriteObjectUrl(asset)
        setState({ url, outcome: { src: objectUrl, failed: false } })
      })
      .catch(() => {
        if (cancelled) return
        setState({ url, outcome: { src: null, failed: true } })
      })

    return () => {
      cancelled = true
      // This consumer minted `objectUrl` for itself; revoking it here can never
      // blank another mounted consumer (each minted its own from the shared
      // asset). If cleanup runs before the fetch settles, `cancelled` short-
      // circuits the `.then` so no orphan blob is ever created.
      if (objectUrl) revokePetSpriteObjectUrl(objectUrl)
    }
  }, [url])

  if (!url) return { src: null, loading: false, failed: false }
  if (state.url === url) {
    return {
      src: state.outcome.src,
      loading: false,
      failed: state.outcome.failed,
    }
  }
  // `url` changed and the effect hasn't resolved the new one yet.
  return { src: null, loading: true, failed: false }
}

/** Test-only: drop cached asset data so module state doesn't leak across tests. */
export function __resetMarketplaceAssetCacheForTests(): void {
  assetCache.clear()
}
