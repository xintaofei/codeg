"use client"

const LAYOUT_MODE_KEY = "workspace:layout-mode"

export type WorkspaceLayoutMode = "fusion" | "files"

const DEFAULT_WORKSPACE_LAYOUT_MODE: WorkspaceLayoutMode = "fusion"

export function loadLayoutMode(): WorkspaceLayoutMode {
  if (typeof window === "undefined") return DEFAULT_WORKSPACE_LAYOUT_MODE
  try {
    const raw = localStorage.getItem(LAYOUT_MODE_KEY)
    if (raw === "fusion" || raw === "files") return raw
  } catch {
    /* ignore */
  }
  return DEFAULT_WORKSPACE_LAYOUT_MODE
}

export function saveLayoutMode(value: WorkspaceLayoutMode): void {
  if (typeof window === "undefined") return
  try {
    localStorage.setItem(LAYOUT_MODE_KEY, value)
  } catch {
    /* ignore */
  }
}
