import { describe, expect, it } from "vitest"
import { appendInstallLogLine, MAX_INSTALL_LOG_LINES } from "./install-stream"

describe("appendInstallLogLine", () => {
  it("appends while below the cap", () => {
    expect(appendInstallLogLine(["a"], "b")).toEqual(["a", "b"])
  })

  it("caps at MAX_INSTALL_LOG_LINES, keeping the most recent lines", () => {
    let logs: string[] = []
    for (let i = 0; i < MAX_INSTALL_LOG_LINES + 250; i++) {
      logs = appendInstallLogLine(logs, `line-${i}`)
    }
    expect(logs).toHaveLength(MAX_INSTALL_LOG_LINES)
    // The first 250 lines were evicted; the tail survives.
    expect(logs[0]).toBe("line-250")
    expect(logs[logs.length - 1]).toBe(`line-${MAX_INSTALL_LOG_LINES + 249}`)
  })

  it("does not mutate the input array", () => {
    const input = ["a"]
    appendInstallLogLine(input, "b")
    expect(input).toEqual(["a"])
  })
})
