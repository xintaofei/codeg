import { useEffect, useState } from "react"

import { fetchWorkspaceBgMarketAsset } from "@/lib/workspace-background-market"
import {
  createBackgroundObjectUrl,
  revokeBackgroundObjectUrl,
  type BackgroundAsset,
} from "@/lib/workspace-background"

// Cache fetched *asset data* per URL (thumbs are immutable content-addressed
// paths) — NOT the blob URL. Paging back / reopening the dialog resolves
// without another wallhaven fetch. Each consumer mints its own blob URL and
// revokes it on unmount, so a shared entry can never be revoked out from
// under a still-mounted consumer.
const assetCache = new Map<string, Promise<BackgroundAsset>>()

function loadAsset(url: string): Promise<BackgroundAsset> {
  const existing = assetCache.get(url)
  if (existing) return existing

  const promise = fetchWorkspaceBgMarketAsset(url)
  assetCache.set(url, promise)
  // Don't cache a rejection — a transient network blip stays retryable. The
  // eviction is identity-guarded so a superseded request's late failure
  // can't evict a newer entry.
  promise.catch(() => {
    if (assetCache.get(url) === promise) assetCache.delete(url)
  })
  return promise
}

export interface ProxiedThumb {
  /** Blob URL for the proxied thumbnail, or `null` while loading / on failure. */
  src: string | null
  loading: boolean
  failed: boolean
}

interface Outcome {
  src: string | null
  failed: boolean
}

/**
 * Resolve a wallhaven thumbnail URL to a locally-served blob URL by proxying
 * the bytes through the backend (`background_market_asset`) — the webview
 * can't reach th.wallhaven.cc directly on some networks, so market cards
 * render wherever the listing loads. Keyed by URL; state is only written
 * from async callbacks, never synchronously in the effect body.
 */
export function useProxiedBackgroundThumb(url: string): ProxiedThumb {
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
        objectUrl = createBackgroundObjectUrl(asset)
        setState({ url, outcome: { src: objectUrl, failed: false } })
      })
      .catch(() => {
        if (cancelled) return
        setState({ url, outcome: { src: null, failed: true } })
      })

    return () => {
      cancelled = true
      if (objectUrl) revokeBackgroundObjectUrl(objectUrl)
    }
  }, [url])

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
export function __resetBackgroundThumbCacheForTests(): void {
  assetCache.clear()
}
