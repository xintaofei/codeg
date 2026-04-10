"use client"

import { useEffect, useRef, useState } from "react"
import { subscribe } from "@/lib/platform"
import {
  APPEARANCE_UPDATED_EVENT,
  getCodeFontSize,
} from "@/lib/appearance-settings"
import {
  terminalSpawn,
  terminalWrite,
  terminalResize,
  terminalKill,
} from "@/lib/api"
import type { TerminalEvent } from "@/lib/types"
import type { ITheme } from "@xterm/xterm"

const DARK_THEME: ITheme = {
  background: "#1a1a1a",
  foreground: "#e0e0e0",
  cursor: "#e0e0e0",
  cursorAccent: "#1a1a1a",
  selectionBackground: "#444444",
  black: "#1a1a1a",
  red: "#f87171",
  green: "#4ade80",
  yellow: "#facc15",
  blue: "#60a5fa",
  magenta: "#c084fc",
  cyan: "#22d3ee",
  white: "#e0e0e0",
  brightBlack: "#737373",
  brightRed: "#fca5a5",
  brightGreen: "#86efac",
  brightYellow: "#fde68a",
  brightBlue: "#93c5fd",
  brightMagenta: "#d8b4fe",
  brightCyan: "#67e8f9",
  brightWhite: "#ffffff",
}

const LIGHT_THEME: ITheme = {
  background: "#ffffff",
  foreground: "#1a1a1a",
  cursor: "#1a1a1a",
  cursorAccent: "#ffffff",
  selectionBackground: "#b4d5fe",
  black: "#1a1a1a",
  red: "#dc2626",
  green: "#16a34a",
  yellow: "#ca8a04",
  blue: "#2563eb",
  magenta: "#9333ea",
  cyan: "#0891b2",
  white: "#e5e5e5",
  brightBlack: "#a3a3a3",
  brightRed: "#ef4444",
  brightGreen: "#22c55e",
  brightYellow: "#eab308",
  brightBlue: "#3b82f6",
  brightMagenta: "#a855f7",
  brightCyan: "#06b6d4",
  brightWhite: "#ffffff",
}

function isDarkMode() {
  return document.documentElement.classList.contains("dark")
}

function resolveBackgroundColor(
  element: HTMLElement | null | undefined
): string | null {
  let current = element
  while (current) {
    const color = getComputedStyle(current).backgroundColor
    if (color && color !== "transparent" && color !== "rgba(0, 0, 0, 0)") {
      return color
    }
    current = current.parentElement
  }
  return null
}

function getTerminalTheme(container: HTMLDivElement | null): ITheme {
  const baseTheme = isDarkMode() ? DARK_THEME : LIGHT_THEME
  const background = resolveBackgroundColor(container)
  if (!background) return baseTheme

  return {
    ...baseTheme,
    background,
    cursorAccent: background,
  }
}

interface TerminalViewProps {
  terminalId: string
  workingDir: string
  initialCommand?: string
  isActive: boolean
  isVisible: boolean
  onProcessExited?: (terminalId: string) => void
}

export function TerminalView({
  terminalId,
  workingDir,
  initialCommand,
  isActive,
  isVisible,
  onProcessExited,
}: TerminalViewProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const fitAddonRef = useRef<{ fit: () => void } | null>(null)
  const termRef = useRef<{ focus: () => void } | null>(null)
  const lastResizeRef = useRef<{ cols: number; rows: number } | null>(null)
  const isActiveRef = useRef(isActive)
  const isVisibleRef = useRef(isVisible)
  const onProcessExitedRef = useRef(onProcessExited)
  const [loading, setLoading] = useState(true)

  useEffect(() => {
    isActiveRef.current = isActive
    isVisibleRef.current = isVisible
  }, [isActive, isVisible])

  useEffect(() => {
    onProcessExitedRef.current = onProcessExited
  }, [onProcessExited])

  useEffect(() => {
    let cancelled = false
    let cleanup: (() => void) | undefined

    async function init() {
      const { Terminal } = await import("@xterm/xterm")
      const { FitAddon } = await import("@xterm/addon-fit")
      const { WebLinksAddon } = await import("@xterm/addon-web-links")

      if (cancelled || !containerRef.current) return

      const fitAddon = new FitAddon()
      const webLinksAddon = new WebLinksAddon()

      const term = new Terminal({
        cursorBlink: true,
        fontSize: getCodeFontSize(),
        fontFamily: "Menlo, Monaco, 'Courier New', monospace",
        theme: getTerminalTheme(containerRef.current),
        allowProposedApi: true,
      })

      term.loadAddon(fitAddon)
      term.loadAddon(webLinksAddon)
      term.open(containerRef.current)

      fitAddonRef.current = fitAddon
      termRef.current = term

      // Watch <html> class changes for theme switching
      const themeObserver = new MutationObserver(() => {
        term.options.theme = getTerminalTheme(containerRef.current)
      })
      themeObserver.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["class"],
      })

      const onAppearanceUpdate = () => {
        term.options.fontSize = getCodeFontSize()
        fitAddon.fit()
      }
      window.addEventListener(APPEARANCE_UPDATED_EVENT, onAppearanceUpdate)

      // Send input to PTY
      const onDataDisposable = term.onData((data: string) => {
        // Some apps toggle focus reporting; don't leak focus in/out sequences
        // into the shell prompt when tabs are switched.
        if (data === "\x1b[I" || data === "\x1b[O") return
        terminalWrite(terminalId, data).catch(() => {})
      })

      // Debounced resize — avoid flooding IPC during drag
      let resizeTimer: ReturnType<typeof setTimeout> | null = null
      const onResizeDisposable = term.onResize(
        ({ cols, rows }: { cols: number; rows: number }) => {
          const last = lastResizeRef.current
          if (last && last.cols === cols && last.rows === rows) return
          lastResizeRef.current = { cols, rows }
          if (resizeTimer) clearTimeout(resizeTimer)
          resizeTimer = setTimeout(() => {
            terminalResize(terminalId, cols, rows).catch(() => {})
          }, 50)
        }
      )

      // Subscribe to events BEFORE spawning so no initial output is lost
      const unlisten = await subscribe<TerminalEvent>(
        `terminal://output/${terminalId}`,
        (payload) => {
          term.write(payload.data)
        }
      )

      const unlistenExit = await subscribe<TerminalEvent>(
        `terminal://exit/${terminalId}`,
        () => {
          onProcessExitedRef.current?.(terminalId)
          term.write("\r\n\x1b[90m[Process exited]\x1b[0m\r\n")
        }
      )

      if (cancelled) {
        window.removeEventListener(APPEARANCE_UPDATED_EVENT, onAppearanceUpdate)
        themeObserver.disconnect()
        onDataDisposable.dispose()
        onResizeDisposable.dispose()
        unlisten()
        unlistenExit()
        term.dispose()
        return
      }

      // Spawn the terminal AFTER subscribing to events
      try {
        await terminalSpawn(workingDir, initialCommand, terminalId)
      } catch (err) {
        onProcessExitedRef.current?.(terminalId)
        term.write(`\r\n\x1b[31m[Failed to start terminal: ${err}]\x1b[0m\r\n`)
      } finally {
        if (!cancelled) setLoading(false)
      }

      // If unmounted while spawn was in flight, clean up the spawned PTY
      if (cancelled) {
        terminalKill(terminalId).catch(() => {})
        window.removeEventListener(APPEARANCE_UPDATED_EVENT, onAppearanceUpdate)
        themeObserver.disconnect()
        onDataDisposable.dispose()
        onResizeDisposable.dispose()
        unlisten()
        unlistenExit()
        term.dispose()
        return
      }

      const fitIfReady = () => {
        const el = containerRef.current
        if (!el) return
        if (!isActiveRef.current || !isVisibleRef.current) return
        if (el.clientWidth <= 0 || el.clientHeight <= 0) return
        fitAddon.fit()
      }

      // Only fit when terminal is actually visible/active.
      requestAnimationFrame(() => {
        if (!cancelled) fitIfReady()
      })

      // Debounced fit on container resize while active
      let fitTimer: ReturnType<typeof setTimeout> | null = null
      const resizeObserver = new ResizeObserver(() => {
        if (fitTimer) clearTimeout(fitTimer)
        fitTimer = setTimeout(() => {
          fitIfReady()
        }, 30)
      })
      resizeObserver.observe(containerRef.current)

      cleanup = () => {
        if (resizeTimer) clearTimeout(resizeTimer)
        if (fitTimer) clearTimeout(fitTimer)
        window.removeEventListener(APPEARANCE_UPDATED_EVENT, onAppearanceUpdate)
        themeObserver.disconnect()
        onDataDisposable.dispose()
        onResizeDisposable.dispose()
        unlisten()
        unlistenExit()
        resizeObserver.disconnect()
        term.dispose()
        fitAddonRef.current = null
        termRef.current = null
        lastResizeRef.current = null
      }
    }

    init()

    return () => {
      cancelled = true
      cleanup?.()
    }
  }, [terminalId, workingDir, initialCommand])

  // Refit and focus when becoming active or panel becomes visible
  useEffect(() => {
    if (isActive && isVisible) {
      requestAnimationFrame(() => {
        const el = containerRef.current
        if (el && el.clientWidth > 0 && el.clientHeight > 0) {
          fitAddonRef.current?.fit()
        }
        termRef.current?.focus()
      })
    }
  }, [isActive, isVisible])

  return (
    <div
      className="absolute inset-0 h-full w-full p-2"
      style={{
        visibility: isActive ? "visible" : "hidden",
        pointerEvents: isActive ? "auto" : "none",
      }}
      aria-hidden={!isActive}
    >
      <div ref={containerRef} className="h-full w-full" />
      {loading && isActive && (
        <div className="absolute inset-0 flex items-center justify-center bg-background/80">
          <div className="flex items-center gap-2 text-sm text-muted-foreground">
            <svg
              className="h-4 w-4 animate-spin"
              viewBox="0 0 24 24"
              fill="none"
            >
              <circle
                className="opacity-25"
                cx="12"
                cy="12"
                r="10"
                stroke="currentColor"
                strokeWidth="4"
              />
              <path
                className="opacity-75"
                fill="currentColor"
                d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z"
              />
            </svg>
            <span>Starting terminal...</span>
          </div>
        </div>
      )}
    </div>
  )
}
