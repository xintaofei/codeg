"use client"

import { useSyncExternalStore } from "react"
import {
  loadAutoReplySettings,
  saveAutoReplySettings,
} from "./storage"
import type { AutoReplySettings } from "./types"

let cached: AutoReplySettings | null = null
const listeners = new Set<() => void>()

function getSnapshot(): AutoReplySettings {
  if (!cached) {
    cached = loadAutoReplySettings()
  }
  return cached
}

function getServerSnapshot(): AutoReplySettings {
  // SSR: always default builtins without touching localStorage.
  return loadAutoReplySettings()
}

function emit() {
  for (const listener of listeners) listener()
}

export function subscribeAutoReplySettings(listener: () => void): () => void {
  listeners.add(listener)
  return () => {
    listeners.delete(listener)
  }
}

export function getAutoReplySettings(): AutoReplySettings {
  return getSnapshot()
}

export function updateAutoReplySettings(
  next:
    | AutoReplySettings
    | ((prev: AutoReplySettings) => AutoReplySettings)
): AutoReplySettings {
  const prev = getSnapshot()
  const resolved = typeof next === "function" ? next(prev) : next
  saveAutoReplySettings(resolved)
  cached = loadAutoReplySettings()
  emit()
  return cached
}

export function useAutoReplySettings(): AutoReplySettings {
  return useSyncExternalStore(
    subscribeAutoReplySettings,
    getSnapshot,
    getServerSnapshot
  )
}

/** Test helper: drop module cache so the next read reloads from storage. */
export function __resetAutoReplySettingsStoreForTests(): void {
  cached = null
}
