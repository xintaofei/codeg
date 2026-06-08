import { describe, it, expect } from "vitest"
import {
  flushRetryDelayMs,
  forkSendBlockedByQueue,
  QUEUE_FLUSH_RETRY_BACKOFF_MS,
  shouldQueueDirectSend,
} from "./queue-flush"

describe("flushRetryDelayMs", () => {
  it("returns 0 when no bounce has happened (lastBounceAt = 0)", () => {
    expect(flushRetryDelayMs(10_000, 0)).toBe(0)
  })

  it("returns 0 once the backoff window has fully elapsed", () => {
    const now = 10_000
    expect(flushRetryDelayMs(now, now - QUEUE_FLUSH_RETRY_BACKOFF_MS)).toBe(0)
    expect(
      flushRetryDelayMs(now, now - QUEUE_FLUSH_RETRY_BACKOFF_MS - 500)
    ).toBe(0)
  })

  it("returns the remaining backoff for a recent bounce (rate-limits retries)", () => {
    const now = 10_000
    // Bounced 200ms ago → wait the rest of the window.
    expect(flushRetryDelayMs(now, now - 200, 1000)).toBe(800)
    // Bounced this instant → wait the full window.
    expect(flushRetryDelayMs(now, now, 1000)).toBe(1000)
  })

  it("clamps to the backoff window (skewed/future bounce → full backoff, never negative)", () => {
    // A future/skewed bounce timestamp is treated as just-bounced — full
    // backoff, never a negative or unbounded delay.
    expect(flushRetryDelayMs(10_000, 20_000, 1000)).toBe(1000)
    expect(flushRetryDelayMs(10_000, 9_500, 1000)).toBe(500)
  })
})

describe("shouldQueueDirectSend", () => {
  it("tail-routes a direct send when the queue is non-empty (preserve FIFO)", () => {
    expect(shouldQueueDirectSend(false, 2)).toBe(true)
    expect(shouldQueueDirectSend(false, 1)).toBe(true)
  })

  it("sends a direct send immediately when the queue is empty", () => {
    expect(shouldQueueDirectSend(false, 0)).toBe(false)
  })

  it("never tail-routes the auto-flush itself (it is draining the queue)", () => {
    expect(shouldQueueDirectSend(true, 5)).toBe(false)
    expect(shouldQueueDirectSend(true, 0)).toBe(false)
  })
})

describe("forkSendBlockedByQueue", () => {
  it("blocks fork-send while the queue is non-empty (fork must not jump the queue)", () => {
    expect(forkSendBlockedByQueue(1)).toBe(true)
    expect(forkSendBlockedByQueue(3)).toBe(true)
  })

  it("allows fork-send when the queue is empty", () => {
    expect(forkSendBlockedByQueue(0)).toBe(false)
  })
})
