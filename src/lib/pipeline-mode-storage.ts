"use client"

/**
 * Storage helpers for persisting the composer's pipeline mode selection
 * ("single" | "duet" | "team" | "custom") per folder to localStorage.
 *
 * Follows the contract key format: "codeg.pipeline.mode.<folderId>"
 * and defaults to "single" if no value or an invalid value is present.
 */

import type { PipelineModeKey } from "@/lib/types"

export const DEFAULT_PIPELINE_MODE: PipelineModeKey = "single"

export const PIPELINE_MODE_KEYS: readonly PipelineModeKey[] = [
  "single",
  "duet",
  "team",
  "custom",
] as const

/**
 * Type guard for PipelineModeKey.
 */
export function isPipelineModeKey(value: unknown): value is PipelineModeKey {
  return (
    typeof value === "string" &&
    (PIPELINE_MODE_KEYS as readonly string[]).includes(value)
  )
}

/**
 * Resolve the localStorage key for the given folderId.
 * Key format: "codeg.pipeline.mode.<folderId>" or "codeg.pipeline.mode.global"
 */
export function getPipelineModeStorageKey(folderId?: number | null): string {
  if (folderId != null) {
    return `codeg.pipeline.mode.${folderId}`
  }
  return "codeg.pipeline.mode.global"
}

/**
 * Read the saved pipeline mode from localStorage.
 * Returns null on SSR, missing entry, or error; callers can use ?? DEFAULT_PIPELINE_MODE.
 */
export function loadPipelineMode(
  folderId?: number | null
): PipelineModeKey | null {
  if (typeof window === "undefined") {
    return null
  }
  try {
    const key = getPipelineModeStorageKey(folderId)
    const raw = localStorage.getItem(key)
    if (raw && isPipelineModeKey(raw)) {
      return raw
    }
    return null
  } catch {
    return null
  }
}

/**
 * Save the pipeline mode selection to localStorage.
 * Failures (e.g. QuotaExceeded or disabled storage) are caught silently.
 */
export function savePipelineMode(
  mode: PipelineModeKey,
  folderId?: number | null
): void {
  if (typeof window === "undefined") {
    return
  }
  try {
    const key = getPipelineModeStorageKey(folderId)
    localStorage.setItem(key, mode)
  } catch {
    /* ignore */
  }
}

/**
 * Remove the pipeline mode entry from localStorage.
 */
export function clearPipelineMode(folderId?: number | null): void {
  if (typeof window === "undefined") {
    return
  }
  try {
    const key = getPipelineModeStorageKey(folderId)
    localStorage.removeItem(key)
  } catch {
    /* ignore */
  }
}
