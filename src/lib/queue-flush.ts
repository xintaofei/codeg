/**
 * Rate-limiting for the conversation message-queue auto-flush.
 *
 * When a queued message is sent but the backend rejects it because a turn is
 * already in flight (TurnBusyError), the draft is re-queued. The queue's
 * level-triggered flush would otherwise re-send it immediately — and if the
 * backend stays busy while the client still believes it is idle (the
 * "prompting" broadcast is delayed or missed), that becomes a retry storm of
 * one bounced send per round-trip. This backoff bounds the retry RATE.
 */

/** Minimum gap between auto-flush retries after a bounced (busy) send. */
export const QUEUE_FLUSH_RETRY_BACKOFF_MS = 1000

/**
 * Milliseconds to wait before the next queue-flush attempt, given `now` and the
 * timestamp of the last bounce (`lastBounceAt`, 0 if none). Returns 0 when no
 * bounce has happened recently — a normal queued message flushes promptly; only
 * a just-bounced retry is delayed.
 */
export function flushRetryDelayMs(
  now: number,
  lastBounceAt: number,
  backoffMs: number = QUEUE_FLUSH_RETRY_BACKOFF_MS
): number {
  // Clamp elapsed at 0 so a skewed/future bounce timestamp is treated as
  // just-bounced (full backoff) rather than yielding an unbounded delay. The
  // result is always within [0, backoffMs].
  const elapsed = Math.max(0, now - lastBounceAt)
  if (elapsed >= backoffMs) return 0
  return backoffMs - elapsed
}

/**
 * Whether a send should be routed to the TAIL of the message queue instead of
 * being sent immediately, to preserve FIFO order.
 *
 * The shared send handler serves two callers: the queue auto-flush (which
 * dequeued the head and must send it now — `fromQueueFlush`), and direct input
 * sends. A direct send issued while the queue is non-empty must NOT jump ahead
 * of already-queued items — it belongs at the tail. The auto-flush always sends
 * (it IS draining the queue), so it never tail-routes.
 */
export function shouldQueueDirectSend(
  fromQueueFlush: boolean,
  queueLength: number
): boolean {
  return !fromQueueFlush && queueLength > 0
}

/**
 * Whether a fork-and-send must be blocked because the message queue is
 * non-empty. Fork is an immediate session side effect (it re-points the live
 * session), so it cannot run while drafts are queued for the CURRENT session —
 * the queued items would otherwise flush onto the forked session, i.e. the fork
 * would jump ahead of the queue. The user drains/clears the queue first.
 */
export function forkSendBlockedByQueue(queueLength: number): boolean {
  return queueLength > 0
}
