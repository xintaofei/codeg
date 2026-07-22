import { beforeEach, describe, expect, it } from "vitest"
import {
  clearCodegToken,
  consumeCodegTokenFromFragment,
  getCodegToken,
  setCodegToken,
} from "./web-auth"

beforeEach(() => {
  localStorage.clear()
  window.history.replaceState(null, "", "/")
})

describe("web auth token storage", () => {
  it("stores and clears the token through the shared helpers", () => {
    setCodegToken("secret")
    expect(getCodegToken()).toBe("secret")

    clearCodegToken()
    expect(getCodegToken()).toBe("")
  })

  it("consumes and decodes a launcher token from the URL fragment", () => {
    window.history.replaceState(
      null,
      "",
      "/workspace?tab=chat#codeg_token=secret%2Ftoken%2Bvalue%3D"
    )

    expect(consumeCodegTokenFromFragment()).toBe("secret/token+value=")
    expect(window.location.pathname).toBe("/workspace")
    expect(window.location.search).toBe("?tab=chat")
    expect(window.location.hash).toBe("")
  })

  it("preserves unrelated fragment parameters and consumes only once", () => {
    window.history.replaceState(
      null,
      "",
      "/#view=diff&codeg_token=secret&line=12"
    )

    expect(consumeCodegTokenFromFragment()).toBe("secret")
    expect(window.location.hash).toBe("#view=diff&line=12")
    expect(consumeCodegTokenFromFragment()).toBeNull()
  })

  it("leaves fragments without a launcher token unchanged", () => {
    window.history.replaceState(null, "", "/#view=files")

    expect(consumeCodegTokenFromFragment()).toBeNull()
    expect(window.location.hash).toBe("#view=files")
  })
})
