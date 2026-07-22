import { render, waitFor } from "@testing-library/react"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

const mocks = vi.hoisted(() => ({
  replace: vi.fn(),
  isDesktop: vi.fn(() => false),
}))

vi.mock("next/navigation", () => ({
  useRouter: () => ({ replace: mocks.replace }),
}))

vi.mock("@/lib/platform", () => ({
  isDesktop: mocks.isDesktop,
}))

import Page from "./page"

const response = (status: number) =>
  ({ ok: status >= 200 && status < 300, status }) as Response

beforeEach(() => {
  mocks.replace.mockClear()
  mocks.isDesktop.mockReturnValue(false)
  localStorage.clear()
  window.history.replaceState(null, "", "/")
  vi.stubGlobal("fetch", vi.fn())
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe("web root authentication", () => {
  it("validates a launcher token, stores it, and scrubs it from the URL", async () => {
    window.history.replaceState(
      null,
      "",
      "/#codeg_token=secret%2Ftoken%2Bvalue%3D&view=diff"
    )
    vi.mocked(fetch).mockResolvedValue(response(200))

    render(<Page />)

    await waitFor(() =>
      expect(mocks.replace).toHaveBeenCalledWith("/workspace")
    )
    expect(fetch).toHaveBeenCalledWith(
      "/api/health",
      expect.objectContaining({
        headers: expect.objectContaining({
          Authorization: "Bearer secret/token+value=",
        }),
      })
    )
    expect(localStorage.getItem("codeg_token")).toBe("secret/token+value=")
    expect(window.location.hash).toBe("#view=diff")
  })

  it("uses a launcher token in preference to a previously stored token", async () => {
    localStorage.setItem("codeg_token", "old-token")
    window.history.replaceState(null, "", "/#codeg_token=new-token")
    vi.mocked(fetch).mockResolvedValue(response(200))

    render(<Page />)

    await waitFor(() =>
      expect(mocks.replace).toHaveBeenCalledWith("/workspace")
    )
    expect(fetch).toHaveBeenCalledWith(
      "/api/health",
      expect.objectContaining({
        headers: expect.objectContaining({
          Authorization: "Bearer new-token",
        }),
      })
    )
    expect(localStorage.getItem("codeg_token")).toBe("new-token")
  })

  it("clears a rejected launcher token and returns to login", async () => {
    localStorage.setItem("codeg_token", "old-token")
    window.history.replaceState(null, "", "/#codeg_token=invalid-token")
    vi.mocked(fetch).mockResolvedValue(response(401))

    render(<Page />)

    await waitFor(() => expect(mocks.replace).toHaveBeenCalledWith("/login"))
    expect(localStorage.getItem("codeg_token")).toBeNull()
    expect(window.location.hash).toBe("")
  })

  it("returns to login without calling the server when no token exists", () => {
    render(<Page />)

    expect(mocks.replace).toHaveBeenCalledWith("/login")
    expect(fetch).not.toHaveBeenCalled()
  })

  it("keeps a stored token during a transient network failure", async () => {
    localStorage.setItem("codeg_token", "stored-token")
    vi.mocked(fetch).mockRejectedValue(new TypeError("Failed to fetch"))

    render(<Page />)

    await waitFor(() =>
      expect(mocks.replace).toHaveBeenCalledWith("/workspace")
    )
    expect(localStorage.getItem("codeg_token")).toBe("stored-token")
  })

  it("skips web authentication in the desktop runtime", () => {
    mocks.isDesktop.mockReturnValue(true)

    render(<Page />)

    expect(mocks.replace).toHaveBeenCalledWith("/workspace")
    expect(fetch).not.toHaveBeenCalled()
  })
})
