"use client"

import { useEffect, useRef, useState } from "react"
import { subscribe } from "@/lib/platform"
import {
  terminalSpawn,
  terminalWrite,
  terminalResize,
  terminalKill,
} from "@/lib/api"
import { createWriteQueue } from "@/lib/terminal/write-queue"
import { useZoomLevel, useTerminalFont } from "@/hooks/use-appearance"
import { detectPlatform } from "@/hooks/use-platform"
import type { TerminalEvent } from "@/lib/types"
import type {
  ITerminalAddon,
  ITheme,
  Terminal as XTermTerminal,
} from "@xterm/xterm"

function computeTerminalFontSize(base: number, zoomLevel: number): number {
  return Math.round((base * zoomLevel) / 100)
}

type DisposableAddon = ITerminalAddon & { dispose: () => void }

/** 惰性加载 @xterm/addon-ligatures（仅终端连字需要，且对系统字体可能无效）。 */
async function enableTerminalLigatures(
  term: XTermTerminal,
  ref: { current: DisposableAddon | null },
  isCurrent: () => boolean
) {
  if (ref.current) return
  try {
    const { LigaturesAddon } = await import("@xterm/addon-ligatures")
    // 动态 import resolve 后重新校验三件事，否则会有竞态：
    // 1) isCurrent()：终端仍是当前实例且连字仍需开启（覆盖「import 期间被销毁/重建」
    //    以及「import 期间用户又关掉连字」两种情况）；
    // 2) ref.current 仍为空：覆盖「并发两次 enable 都通过了 await 前检查」——
    //    校验到赋值之间无 await，先到者占位后，后到者在此返回，避免重复挂载。
    if (!isCurrent() || ref.current) return
    const addon = new LigaturesAddon() as unknown as DisposableAddon
    term.loadAddon(addon)
    ref.current = addon
  } catch {
    // 加载失败时静默降级
  }
}

function disableTerminalLigatures(ref: { current: DisposableAddon | null }) {
  try {
    ref.current?.dispose()
  } catch {
    // ignore
  }
  ref.current = null
}

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

// #1a1a1a / #ffffff 的 alpha 0 版本。工作区背景图开启时用它替换终端背景色，让画布透出
// 所属 ws-surface 面板的磨砂表面。RGB 保持与对应主题背景色一致，故 xterm 由背景色派生
// 的反显（inverse video）字色 color.opaque(bg) 仍是原主题背景色，不会随透明背景变黑。
const DARK_TRANSPARENT_BACKGROUND = "rgba(26, 26, 26, 0)"
const LIGHT_TRANSPARENT_BACKGROUND = "rgba(255, 255, 255, 0)"

function isDarkMode() {
  return document.documentElement.classList.contains("dark")
}

// 工作区背景图是否开启（<html data-workspace-bg="on">，由 AppearanceProvider 设置）。
function isWorkspaceBgOn() {
  return document.documentElement.getAttribute("data-workspace-bg") === "on"
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
  const dark = isDarkMode()
  const baseTheme = dark ? DARK_THEME : LIGHT_THEME

  // 背景图开启：终端画布透明，透出所属 ws-surface 面板的磨砂表面（跟随面板不透明度滑块），
  // 而非用不透明色盖住背景图。只改 background；cursorAccent 保留主题不透明色，块状光标下的
  // 字符才不会随透明背景一起消失。
  if (isWorkspaceBgOn()) {
    return {
      ...baseTheme,
      background: dark
        ? DARK_TRANSPARENT_BACKGROUND
        : LIGHT_TRANSPARENT_BACKGROUND,
    }
  }

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
  shell?: string
  initialCommand?: string
  isActive: boolean
  isVisible: boolean
  onProcessExited?: (terminalId: string) => void
}

export function TerminalView({
  terminalId,
  workingDir,
  shell,
  initialCommand,
  isActive,
  isVisible,
  onProcessExited,
}: TerminalViewProps) {
  const containerRef = useRef<HTMLDivElement>(null)
  const fitAddonRef = useRef<{ fit: () => void } | null>(null)
  const termRef = useRef<XTermTerminal | null>(null)
  const lastResizeRef = useRef<{ cols: number; rows: number } | null>(null)
  const isActiveRef = useRef(isActive)
  const isVisibleRef = useRef(isVisible)
  const onProcessExitedRef = useRef(onProcessExited)
  const { zoomLevel } = useZoomLevel()
  const { terminalFontStack, terminalFontSize, terminalLigatures } =
    useTerminalFont()
  const zoomLevelRef = useRef(zoomLevel)
  const terminalFontRef = useRef(terminalFontStack)
  const terminalSizeRef = useRef(terminalFontSize)
  const terminalLigaturesRef = useRef(terminalLigatures)
  const ligaturesAddonRef = useRef<DisposableAddon | null>(null)
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
        fontSize: computeTerminalFontSize(
          terminalSizeRef.current,
          zoomLevelRef.current
        ),
        fontFamily: terminalFontRef.current,
        theme: getTerminalTheme(containerRef.current),
        // 允许透明背景：背景图开启时 getTerminalTheme 返回 alpha 0 的背景色，透出下方磨砂
        // 面板。当前 DOM 渲染器本就按 CSS 应用背景色（透明即生效，不消费此项）；显式设置是
        // 为将来若改用 canvas/webgl 渲染器时仍保持透明，不致回退成不透明黑块。
        allowTransparency: true,
        allowProposedApi: true,
      })

      term.loadAddon(fitAddon)
      term.loadAddon(webLinksAddon)
      term.open(containerRef.current)

      fitAddonRef.current = fitAddon
      termRef.current = term

      if (terminalLigaturesRef.current) {
        enableTerminalLigatures(
          term,
          ligaturesAddonRef,
          () => termRef.current === term && terminalLigaturesRef.current
        )
      }

      // Ordered single-flight pump for terminal input. Both onData (typed
      // bytes) and the custom-key escape sequences below feed this one queue,
      // so input reaches the PTY in exact type order regardless of transport
      // reordering, and fast bursts coalesce into fewer round-trips. A failed
      // send is dropped, not retried — re-sending an ambiguous write could
      // duplicate already-delivered bytes, worse than a drop in a shell. See
      // lib/terminal/write-queue.ts.
      const writeQueue = createWriteQueue((d) => terminalWrite(terminalId, d))

      // Shell line-editing shortcuts. Sends readline/zle bindings so they
      // work regardless of terminfo.
      //   Alt/Option + ←/→ / Backspace: word-level moves & delete
      //   macOS Cmd + ←/→ / Backspace : line-level moves & clear
      // Uses `e.code` (physical key) to be robust against dead-key layouts on
      // macOS where Option can turn some keys into `key: "Dead"`.
      // AltGr on Windows/Linux is reported as ctrlKey+altKey and is excluded
      // by the `!ctrlKey` guard below.
      const isMac = detectPlatform() === "macos"
      term.attachCustomKeyEventHandler((e) => {
        if (e.type !== "keydown") return true
        // Skip during IME composition to avoid corrupting candidate buffer.
        if (e.isComposing) return true

        const { code, altKey, metaKey, ctrlKey, shiftKey } = e

        const writeSeq = (seq: string) => {
          writeQueue.enqueue(seq)
          e.preventDefault()
          return false
        }

        if (altKey && !ctrlKey && !metaKey && !shiftKey) {
          if (code === "ArrowLeft") return writeSeq("\x1bb")
          if (code === "ArrowRight") return writeSeq("\x1bf")
          if (code === "Backspace") return writeSeq("\x1b\x7f")
        }

        if (isMac && metaKey && !altKey && !ctrlKey && !shiftKey) {
          if (code === "ArrowLeft") return writeSeq("\x01")
          if (code === "ArrowRight") return writeSeq("\x05")
          if (code === "Backspace") return writeSeq("\x15")
        }

        return true
      })

      // Watch <html> for theme (class) and workspace-background (data-workspace-bg)
      // switching — both change what getTerminalTheme returns (dark/light palette,
      // and transparent-vs-opaque background), so re-push the theme on either.
      const themeObserver = new MutationObserver(() => {
        term.options.theme = getTerminalTheme(containerRef.current)
      })
      themeObserver.observe(document.documentElement, {
        attributes: true,
        attributeFilter: ["class", "data-workspace-bg"],
      })

      // Send input to PTY
      const onDataDisposable = term.onData((data: string) => {
        // Some apps toggle focus reporting; don't leak focus in/out sequences
        // into the shell prompt when tabs are switched.
        if (data === "\x1b[I" || data === "\x1b[O") return
        writeQueue.enqueue(data)
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
          // PTY is gone — stop the input pump (the reliable terminal-gone
          // signal; the queue's error-string match is only a fast-path).
          writeQueue.dispose()
          onProcessExitedRef.current?.(terminalId)
          term.write("\r\n\x1b[90m[Process exited]\x1b[0m\r\n")
        }
      )

      if (cancelled) {
        writeQueue.dispose()
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
        await terminalSpawn(workingDir, shell, initialCommand, terminalId)
      } catch (err) {
        onProcessExitedRef.current?.(terminalId)
        term.write(`\r\n\x1b[31m[Failed to start terminal: ${err}]\x1b[0m\r\n`)
      } finally {
        if (!cancelled) setLoading(false)
      }

      // If unmounted while spawn was in flight, clean up the spawned PTY
      if (cancelled) {
        writeQueue.dispose()
        terminalKill(terminalId).catch(() => {})
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
        writeQueue.dispose()
        if (resizeTimer) clearTimeout(resizeTimer)
        if (fitTimer) clearTimeout(fitTimer)
        themeObserver.disconnect()
        onDataDisposable.dispose()
        onResizeDisposable.dispose()
        unlisten()
        unlistenExit()
        resizeObserver.disconnect()
        term.dispose()
        fitAddonRef.current = null
        termRef.current = null
        ligaturesAddonRef.current = null
        lastResizeRef.current = null
      }
    }

    init()

    return () => {
      cancelled = true
      cleanup?.()
    }
  }, [terminalId, workingDir, shell, initialCommand])

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

  // React to zoom / font-family / font-size changes. Updates refs synchronously so
  // async init() always reads the latest values, and pushes them to already-mounted
  // terminals. Double rAF ensures xterm's renderer has recomputed cell metrics
  // before we refit.
  useEffect(() => {
    zoomLevelRef.current = zoomLevel
    terminalFontRef.current = terminalFontStack
    terminalSizeRef.current = terminalFontSize
    const term = termRef.current
    if (!term) return
    term.options.fontFamily = terminalFontStack
    term.options.fontSize = computeTerminalFontSize(terminalFontSize, zoomLevel)
    requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        const el = containerRef.current
        if (el && el.clientWidth > 0 && el.clientHeight > 0) {
          fitAddonRef.current?.fit()
        }
      })
    })
  }, [zoomLevel, terminalFontStack, terminalFontSize])

  // React to the ligature toggle. Lazily loads @xterm/addon-ligatures on enable,
  // disposes it on disable.
  useEffect(() => {
    terminalLigaturesRef.current = terminalLigatures
    const term = termRef.current
    if (!term) return
    if (terminalLigatures) {
      enableTerminalLigatures(
        term,
        ligaturesAddonRef,
        () => termRef.current === term && terminalLigaturesRef.current
      )
    } else {
      disableTerminalLigatures(ligaturesAddonRef)
    }
  }, [terminalLigatures])

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
