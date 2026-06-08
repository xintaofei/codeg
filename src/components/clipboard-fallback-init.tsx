"use client"

import { useEffect } from "react"
import { installClipboardFallback } from "@/lib/utils"

/**
 * Installs a `navigator.clipboard.writeText` fallback for non-secure contexts
 * (the web build served over HTTP/LAN), so third-party copy buttons that lack
 * their own fallback — e.g. Streamdown code blocks — work there. No-op in
 * secure contexts (desktop Tauri, HTTPS, localhost).
 */
export function ClipboardFallbackInit() {
  useEffect(() => {
    installClipboardFallback()
  }, [])
  return null
}
