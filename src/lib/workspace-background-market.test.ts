import { beforeEach, describe, expect, it, vi } from "vitest"

const callMock = vi.fn()

vi.mock("@/lib/transport", () => ({
  getTransport: () => ({ call: callMock }),
}))

import {
  downloadWorkspaceBgMarket,
  fetchWorkspaceBgMarketAsset,
  searchWorkspaceBgMarket,
} from "./workspace-background-market"

beforeEach(() => {
  callMock.mockReset()
})

describe("workspace-background-market transport bindings", () => {
  it("passes camelCase params to background_market_search", async () => {
    callMock.mockResolvedValue({ items: [], page: 2, lastPage: 5 })
    await searchWorkspaceBgMarket({
      query: "mountain",
      category: "anime",
      page: 2,
    })
    expect(callMock).toHaveBeenCalledWith("background_market_search", {
      query: "mountain",
      category: "anime",
      page: 2,
    })
  })

  it("proxies asset fetch through background_market_asset", async () => {
    callMock.mockResolvedValue({ mime: "image/jpeg", dataBase64: "eHg=" })
    await fetchWorkspaceBgMarketAsset("https://th.wallhaven.cc/small/ab/x.jpg")
    expect(callMock).toHaveBeenCalledWith("background_market_asset", {
      url: "https://th.wallhaven.cc/small/ab/x.jpg",
    })
  })

  it("sends url + sourceUrl to background_market_download", async () => {
    callMock.mockResolvedValue(undefined)
    await downloadWorkspaceBgMarket(
      "https://w.wallhaven.cc/full/ab/x.jpg",
      "https://wallhaven.cc/w/x"
    )
    expect(callMock).toHaveBeenCalledWith("background_market_download", {
      url: "https://w.wallhaven.cc/full/ab/x.jpg",
      sourceUrl: "https://wallhaven.cc/w/x",
    })
  })
})
