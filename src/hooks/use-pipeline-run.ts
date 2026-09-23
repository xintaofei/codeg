"use client"

import { useEffect, useState } from "react"
import { pipelineRunStatus, subscribePipelineChanged } from "@/lib/api"
import type { PipelineRun } from "@/lib/types"

/**
 * Hook that subscribes to pipeline run events and maintains the current run state.
 * Updates when the pipeline status changes.
 *
 * @param runId - The ID of the pipeline run to monitor (null to disable)
 * @returns The current PipelineRun state, or null if not found/not monitoring
 */
export function usePipelineRun(runId: number | null): PipelineRun | null {
  const [run, setRun] = useState<PipelineRun | null>(null)

  useEffect(() => {
    if (!runId) {
      return
    }

    let cancelled = false
    let unsubscribe: (() => void) | null = null

    const setup = async () => {
      try {
        // Initial load
        const initialRun = await pipelineRunStatus(runId)
        if (!cancelled) {
          setRun(initialRun)
        }

        // Subscribe to events
        unsubscribe = await subscribePipelineChanged((change) => {
          if (cancelled) return

          // Only update if this event is for our run
          if (
            (change.kind === "run_started" && change.run_id === runId) ||
            (change.kind === "step_started" && change.run_id === runId) ||
            (change.kind === "step_settled" && change.run_id === runId) ||
            (change.kind === "run_settled" && change.run_id === runId)
          ) {
            // Refetch the full status
            void (async () => {
              try {
                const updated = await pipelineRunStatus(runId)
                if (!cancelled) {
                  setRun(updated)
                }
              } catch (e) {
                console.error("[usePipelineRun] failed to fetch status:", e)
              }
            })()
          }
        })
      } catch (e) {
        console.error("[usePipelineRun] setup failed:", e)
      }
    }

    void setup()

    return () => {
      cancelled = true
      if (unsubscribe) {
        unsubscribe()
      }
    }
  }, [runId])

  return run
}
