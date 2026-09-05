// Transport-aware bindings for the wallpaper market (wallhaven.cc) commands.
// Same dual-mode pattern as src/lib/workspace-background.ts: everything goes
// through getTransport().call(...) so one code path serves Tauri (invoke) and
// standalone-server (fetch) modes.

import { getTransport } from "@/lib/transport"
import type { BackgroundAsset } from "@/lib/workspace-background"

// ─── Types ───

/** Category filter mirrored from the Rust `wallhaven_categories` allowlist. */
export const MARKET_CATEGORIES = ["all", "general", "anime", "people"] as const
export type MarketCategory = (typeof MARKET_CATEGORIES)[number]

/** camelCase mirror of the Rust `MarketWallpaperSummary`. */
export type MarketWallpaper = {
  id: string
  thumbUrl: string
  fullUrl: string
  sourceUrl: string
  resolution: string
  category: string
}

/** camelCase mirror of the Rust `MarketSearchPage`. */
export type MarketSearchResult = {
  items: MarketWallpaper[]
  page: number
  lastPage: number
}

// ─── Transport bindings ───

export async function searchWorkspaceBgMarket(input: {
  query: string
  category: MarketCategory
  page: number
}): Promise<MarketSearchResult> {
  return getTransport().call("background_market_search", {
    query: input.query,
    category: input.category,
    page: input.page,
  })
}

export async function fetchWorkspaceBgMarketAsset(
  url: string
): Promise<BackgroundAsset> {
  return getTransport().call("background_market_asset", { url })
}

export async function downloadWorkspaceBgMarket(
  url: string,
  sourceUrl: string
): Promise<void> {
  return getTransport().call("background_market_download", {
    url,
    sourceUrl,
  })
}
