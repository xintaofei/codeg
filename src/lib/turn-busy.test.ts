import { describe, it, expect } from "vitest"
import {
  TurnBusyError,
  isNoActiveTurnRejection,
  isTurnInProgressRejection,
} from "./turn-busy"

describe("isTurnInProgressRejection", () => {
  it("recognizes the bare marker string (Tauri rejection)", () => {
    // AcpError serializes to its Display string; Tauri rejects with it directly.
    expect(
      isTurnInProgressRejection("turn already in progress for this connection")
    ).toBe(true)
  })

  it("recognizes the marker inside a web error message", () => {
    // task_execution_failed(e.to_string()) carries the Display as `message`.
    expect(
      isTurnInProgressRejection({
        code: "task_execution_failed",
        message: "turn already in progress for this connection",
      })
    ).toBe(true)
  })

  it("recognizes the stable web error code (regardless of message)", () => {
    // AppErrorCode::TurnInProgress (HTTP 409) serializes as this code.
    expect(
      isTurnInProgressRejection({ code: "turn_in_progress", message: "" })
    ).toBe(true)
  })

  it("does NOT match unrelated errors", () => {
    expect(isTurnInProgressRejection("process exited unexpectedly")).toBe(false)
    expect(
      isTurnInProgressRejection({
        code: "network_error",
        message: "HTTP 500",
      })
    ).toBe(false)
    expect(isTurnInProgressRejection(null)).toBe(false)
    expect(isTurnInProgressRejection(undefined)).toBe(false)
    expect(isTurnInProgressRejection(42)).toBe(false)
    expect(isTurnInProgressRejection({})).toBe(false)
  })
})

describe("isNoActiveTurnRejection", () => {
  it("recognizes the bare marker string (Tauri AcpError Display)", () => {
    expect(isNoActiveTurnRejection("no active turn to send feedback to")).toBe(
      true
    )
  })

  it("recognizes the marker inside a web error message", () => {
    // The web handler maps NoActiveTurn → InvalidInput, preserving the message.
    expect(
      isNoActiveTurnRejection({
        code: "invalid_input",
        message: "no active turn to send feedback to",
      })
    ).toBe(true)
  })

  it("does NOT match unrelated errors", () => {
    expect(isNoActiveTurnRejection("turn already in progress")).toBe(false)
    expect(isNoActiveTurnRejection({ message: "HTTP 500" })).toBe(false)
    expect(isNoActiveTurnRejection(null)).toBe(false)
    expect(isNoActiveTurnRejection(undefined)).toBe(false)
    expect(isNoActiveTurnRejection({})).toBe(false)
  })
})

describe("TurnBusyError", () => {
  it("is an Error subclass carrying the marker text", () => {
    const e = new TurnBusyError()
    expect(e).toBeInstanceOf(Error)
    expect(e.name).toBe("TurnBusyError")
    // Round-trips back through the recognizer (the same marker text).
    expect(isTurnInProgressRejection(e)).toBe(true)
  })
})
