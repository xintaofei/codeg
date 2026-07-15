"use client"

import { createContext, useCallback, useEffect, useState } from "react"
import {
  THEME_COLORS,
  DEFAULT_THEME_COLOR,
  type ThemeColor,
  ZOOM_LEVELS,
  DEFAULT_ZOOM_LEVEL,
  type ZoomLevel,
} from "@/lib/theme-presets"
import {
  resolveFontStack,
  isValidFontId,
  isValidFontSize,
  DEFAULT_UI_FONT_ID,
  DEFAULT_EDITOR_FONT_ID,
  DEFAULT_TERMINAL_FONT_ID,
  DEFAULT_EDITOR_FONT_SIZE,
  DEFAULT_TERMINAL_FONT_SIZE,
  type FontSize,
} from "@/lib/font-presets"
import {
  STORAGE_KEY_THEME_COLOR,
  STORAGE_KEY_ZOOM_LEVEL,
  STORAGE_KEY_WELCOME_QUICK_ACTIONS,
  STORAGE_KEY_MARKDOWN_PREVIEW_PRESERVE_LINE_BREAKS,
  STORAGE_KEY_UI_FONT,
  STORAGE_KEY_UI_FONT_CUSTOM,
  STORAGE_KEY_UI_FONT_STACK,
  STORAGE_KEY_EDITOR_FONT,
  STORAGE_KEY_EDITOR_FONT_CUSTOM,
  STORAGE_KEY_EDITOR_FONT_SIZE,
  STORAGE_KEY_EDITOR_LIGATURES,
  STORAGE_KEY_EDITOR_WORD_WRAP,
  STORAGE_KEY_TERMINAL_FONT,
  STORAGE_KEY_TERMINAL_FONT_CUSTOM,
  STORAGE_KEY_TERMINAL_FONT_SIZE,
  STORAGE_KEY_TERMINAL_LIGATURES,
} from "@/lib/appearance-script"

function syncTrafficLightPosition(zoom: number) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window))
    return
  import("@/lib/tauri").then((t) =>
    t.updateTrafficLightPosition(zoom).catch(() => {})
  )
}

function syncAppearanceMode(mode: string) {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window))
    return
  import("@/lib/tauri").then((t) =>
    t.updateAppearanceMode(mode).catch(() => {})
  )
}

export type FontSelection = { id: string; custom: string }

type AppearanceContextValue = {
  themeColor: ThemeColor
  setThemeColor: (color: ThemeColor) => void
  zoomLevel: ZoomLevel
  setZoomLevel: (zoom: ZoomLevel) => void
  /** 新会话欢迎页是否显示「模式选择区域」（QuickActions 快捷卡片），默认开启 */
  showWelcomeQuickActions: boolean
  setShowWelcomeQuickActions: (on: boolean) => void
  /** Markdown 文件预览是否将单个物理换行显示为可见换行，默认关闭 */
  markdownPreviewPreserveLineBreaks: boolean
  setMarkdownPreviewPreserveLineBreaks: (on: boolean) => void
  /** 界面字体（普通组件，驱动 --font-sans） */
  uiFont: FontSelection
  setUiFont: (id: string, custom?: string) => void
  /** 编辑器字体（仅作用于代码编辑器 Monaco 的 fontFamily） */
  editorFont: FontSelection
  setEditorFont: (id: string, custom?: string) => void
  /** 终端字体（驱动 xterm fontFamily） */
  terminalFont: FontSelection
  setTerminalFont: (id: string, custom?: string) => void
  editorFontSize: FontSize
  setEditorFontSize: (size: FontSize) => void
  terminalFontSize: FontSize
  setTerminalFontSize: (size: FontSize) => void
  editorLigatures: boolean
  setEditorLigatures: (on: boolean) => void
  /** 编辑器自动换行（作用于代码编辑器 Monaco 的 wordWrap 选项） */
  editorWordWrap: boolean
  setEditorWordWrap: (on: boolean) => void
  terminalLigatures: boolean
  setTerminalLigatures: (on: boolean) => void
}

export const AppearanceContext = createContext<AppearanceContextValue | null>(
  null
)

function persist(key: string, value: string) {
  try {
    localStorage.setItem(key, value)
  } catch {
    // 隐私模式 / 禁用 storage 时静默忽略，本次会话内仍然生效
  }
}

function readStored(key: string): string | null {
  try {
    return localStorage.getItem(key)
  } catch {
    return null
  }
}

function readFontSelection(
  idKey: string,
  customKey: string,
  def: string
): FontSelection {
  if (typeof document === "undefined") return { id: def, custom: "" }
  try {
    const id = localStorage.getItem(idKey)
    const custom = localStorage.getItem(customKey) ?? ""
    return { id: isValidFontId(id) ? (id as string) : def, custom }
  } catch {
    return { id: def, custom: "" }
  }
}

function readFontSize(key: string, def: FontSize): FontSize {
  if (typeof document === "undefined") return def
  try {
    const n = parseInt(localStorage.getItem(key) ?? "", 10)
    return isValidFontSize(n) ? n : def
  } catch {
    return def
  }
}

function readBool(key: string, def: boolean): boolean {
  if (typeof document === "undefined") return def
  try {
    const v = localStorage.getItem(key)
    return v === null ? def : v === "1"
  } catch {
    return def
  }
}

/**
 * AppearanceProvider 管理 themeColor、zoomLevel 与字体偏好。
 *
 * 与 next-themes 完全正交：next-themes 负责 <html class="dark/light">，
 * 这里负责 <html data-theme="...">、<html style="font-size: ...">
 * 以及界面字体变量 --font-sans（编辑器/终端字体只走各自的 Monaco/xterm 选项）。
 *
 * 注意：next-themes 的 attribute 配置必须保持 "class"。如果改为 "data-theme"
 * 会与本 Provider 冲突，导致主题色无法生效。
 */
export function AppearanceProvider({
  children,
}: {
  children: React.ReactNode
}) {
  // 初始值从 DOM 读取（appearance-script.ts 在 hydration 前已经写好），
  // 而不是从 localStorage 读 —— 避免 SSR 与 CSR 不一致导致的双闪烁。
  const [themeColor, setThemeColorState] = useState<ThemeColor>(() => {
    if (typeof document === "undefined") return DEFAULT_THEME_COLOR
    const attr = document.documentElement.getAttribute(
      "data-theme"
    ) as ThemeColor | null
    return attr && (THEME_COLORS as readonly string[]).includes(attr)
      ? attr
      : DEFAULT_THEME_COLOR
  })

  const [zoomLevel, setZoomLevelState] = useState<ZoomLevel>(() => {
    if (typeof document === "undefined") return DEFAULT_ZOOM_LEVEL
    const px = parseFloat(document.documentElement.style.fontSize || "16")
    const level = Math.round((px / 16) * 100) as ZoomLevel
    return (ZOOM_LEVELS as readonly number[]).includes(level)
      ? level
      : DEFAULT_ZOOM_LEVEL
  })

  // 新会话「模式选择区域」显示开关：默认开启，键缺失即回退为 true。
  // QuickActions 仅在欢迎态客户端渲染，此处同步读 localStorage 不会造成首帧闪烁。
  const [showWelcomeQuickActions, setShowWelcomeQuickActionsState] =
    useState<boolean>(() => readBool(STORAGE_KEY_WELCOME_QUICK_ACTIONS, true))
  const [
    markdownPreviewPreserveLineBreaks,
    setMarkdownPreviewPreserveLineBreaksState,
  ] = useState<boolean>(() =>
    readBool(STORAGE_KEY_MARKDOWN_PREVIEW_PRESERVE_LINE_BREAKS, false)
  )

  // 字体偏好的初始值从 localStorage 读 id/custom（视觉已由 inline 脚本就位，
  // 这里只是回填选中态，不会造成闪烁）。
  const [uiFont, setUiFontState] = useState<FontSelection>(() =>
    readFontSelection(
      STORAGE_KEY_UI_FONT,
      STORAGE_KEY_UI_FONT_CUSTOM,
      DEFAULT_UI_FONT_ID
    )
  )
  const [editorFont, setEditorFontState] = useState<FontSelection>(() =>
    readFontSelection(
      STORAGE_KEY_EDITOR_FONT,
      STORAGE_KEY_EDITOR_FONT_CUSTOM,
      DEFAULT_EDITOR_FONT_ID
    )
  )
  const [terminalFont, setTerminalFontState] = useState<FontSelection>(() =>
    readFontSelection(
      STORAGE_KEY_TERMINAL_FONT,
      STORAGE_KEY_TERMINAL_FONT_CUSTOM,
      DEFAULT_TERMINAL_FONT_ID
    )
  )
  const [editorFontSize, setEditorFontSizeState] = useState<FontSize>(() =>
    readFontSize(STORAGE_KEY_EDITOR_FONT_SIZE, DEFAULT_EDITOR_FONT_SIZE)
  )
  const [terminalFontSize, setTerminalFontSizeState] = useState<FontSize>(() =>
    readFontSize(STORAGE_KEY_TERMINAL_FONT_SIZE, DEFAULT_TERMINAL_FONT_SIZE)
  )
  const [editorLigatures, setEditorLigaturesState] = useState<boolean>(() =>
    readBool(STORAGE_KEY_EDITOR_LIGATURES, false)
  )
  const [editorWordWrap, setEditorWordWrapState] = useState<boolean>(() =>
    readBool(STORAGE_KEY_EDITOR_WORD_WRAP, false)
  )
  const [terminalLigatures, setTerminalLigaturesState] = useState<boolean>(() =>
    readBool(STORAGE_KEY_TERMINAL_LIGATURES, false)
  )

  const setThemeColor = useCallback((color: ThemeColor) => {
    setThemeColorState(color)
    document.documentElement.setAttribute("data-theme", color)
    persist(STORAGE_KEY_THEME_COLOR, color)
  }, [])

  const setZoomLevel = useCallback((zoom: ZoomLevel) => {
    setZoomLevelState(zoom)
    document.documentElement.style.fontSize = `${(16 * zoom) / 100}px`
    syncTrafficLightPosition(zoom)
    persist(STORAGE_KEY_ZOOM_LEVEL, String(zoom))
  }, [])

  const setShowWelcomeQuickActions = useCallback((on: boolean) => {
    setShowWelcomeQuickActionsState(on)
    persist(STORAGE_KEY_WELCOME_QUICK_ACTIONS, on ? "1" : "0")
  }, [])

  const setMarkdownPreviewPreserveLineBreaks = useCallback((on: boolean) => {
    setMarkdownPreviewPreserveLineBreaksState(on)
    persist(STORAGE_KEY_MARKDOWN_PREVIEW_PRESERVE_LINE_BREAKS, on ? "1" : "0")
  }, [])

  const setUiFont = useCallback((id: string, custom = "") => {
    setUiFontState({ id, custom })
    const stack = resolveFontStack(id, custom, "sans")
    document.documentElement.style.setProperty("--font-sans", stack)
    persist(STORAGE_KEY_UI_FONT, id)
    persist(STORAGE_KEY_UI_FONT_CUSTOM, custom)
    persist(STORAGE_KEY_UI_FONT_STACK, stack)
  }, [])

  const setEditorFont = useCallback((id: string, custom = "") => {
    setEditorFontState({ id, custom })
    // 编辑器字体只作用于代码编辑器（Monaco），不写任何全局 CSS 变量，
    // 不影响界面与会话消息区（它们跟随 --font-sans）。
    persist(STORAGE_KEY_EDITOR_FONT, id)
    persist(STORAGE_KEY_EDITOR_FONT_CUSTOM, custom)
  }, [])

  const setTerminalFont = useCallback((id: string, custom = "") => {
    setTerminalFontState({ id, custom })
    persist(STORAGE_KEY_TERMINAL_FONT, id)
    persist(STORAGE_KEY_TERMINAL_FONT_CUSTOM, custom)
  }, [])

  const setEditorFontSize = useCallback((size: FontSize) => {
    setEditorFontSizeState(size)
    persist(STORAGE_KEY_EDITOR_FONT_SIZE, String(size))
  }, [])

  const setTerminalFontSize = useCallback((size: FontSize) => {
    setTerminalFontSizeState(size)
    persist(STORAGE_KEY_TERMINAL_FONT_SIZE, String(size))
  }, [])

  const setEditorLigatures = useCallback((on: boolean) => {
    setEditorLigaturesState(on)
    persist(STORAGE_KEY_EDITOR_LIGATURES, on ? "1" : "0")
  }, [])

  const setEditorWordWrap = useCallback((on: boolean) => {
    setEditorWordWrapState(on)
    persist(STORAGE_KEY_EDITOR_WORD_WRAP, on ? "1" : "0")
  }, [])

  const setTerminalLigatures = useCallback((on: boolean) => {
    setTerminalLigaturesState(on)
    persist(STORAGE_KEY_TERMINAL_LIGATURES, on ? "1" : "0")
  }, [])

  // Sync traffic-light position and appearance mode on mount
  useEffect(() => {
    syncTrafficLightPosition(zoomLevel)
    try {
      syncAppearanceMode(localStorage.getItem("theme") ?? "system")
    } catch {
      // localStorage unavailable
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // 仅界面字体需要在 mount 时重新解析并应用 --font-sans，吸收跨版本字体目录变更
  // （inline 脚本写入的是旧版本已解析栈，可能与新目录不一致）。仅在确有漂移时才写，
  // 避免每次加载都触发 localStorage 写入与跨标签页 storage 事件。
  // 编辑器/终端字体只走各自的 Monaco/xterm 选项，不在此落 CSS 变量。
  useEffect(() => {
    const sans = resolveFontStack(uiFont.id, uiFont.custom, "sans")
    if (readStored(STORAGE_KEY_UI_FONT_STACK) !== sans) {
      document.documentElement.style.setProperty("--font-sans", sans)
      persist(STORAGE_KEY_UI_FONT_STACK, sans)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // 跨标签页同步：用户在另一个窗口改了设置时，本窗口实时跟进
  useEffect(() => {
    const FONT_KEYS = new Set<string>([
      STORAGE_KEY_UI_FONT,
      STORAGE_KEY_UI_FONT_CUSTOM,
      STORAGE_KEY_UI_FONT_STACK,
      STORAGE_KEY_EDITOR_FONT,
      STORAGE_KEY_EDITOR_FONT_CUSTOM,
      STORAGE_KEY_EDITOR_FONT_SIZE,
      STORAGE_KEY_EDITOR_LIGATURES,
      STORAGE_KEY_EDITOR_WORD_WRAP,
      STORAGE_KEY_TERMINAL_FONT,
      STORAGE_KEY_TERMINAL_FONT_CUSTOM,
      STORAGE_KEY_TERMINAL_FONT_SIZE,
      STORAGE_KEY_TERMINAL_LIGATURES,
    ])
    const rehydrateFonts = () => {
      const ui = readFontSelection(
        STORAGE_KEY_UI_FONT,
        STORAGE_KEY_UI_FONT_CUSTOM,
        DEFAULT_UI_FONT_ID
      )
      const ed = readFontSelection(
        STORAGE_KEY_EDITOR_FONT,
        STORAGE_KEY_EDITOR_FONT_CUSTOM,
        DEFAULT_EDITOR_FONT_ID
      )
      const tm = readFontSelection(
        STORAGE_KEY_TERMINAL_FONT,
        STORAGE_KEY_TERMINAL_FONT_CUSTOM,
        DEFAULT_TERMINAL_FONT_ID
      )
      setUiFontState(ui)
      setEditorFontState(ed)
      setTerminalFontState(tm)
      setEditorFontSizeState(
        readFontSize(STORAGE_KEY_EDITOR_FONT_SIZE, DEFAULT_EDITOR_FONT_SIZE)
      )
      setTerminalFontSizeState(
        readFontSize(STORAGE_KEY_TERMINAL_FONT_SIZE, DEFAULT_TERMINAL_FONT_SIZE)
      )
      setEditorLigaturesState(readBool(STORAGE_KEY_EDITOR_LIGATURES, false))
      setEditorWordWrapState(readBool(STORAGE_KEY_EDITOR_WORD_WRAP, false))
      setTerminalLigaturesState(readBool(STORAGE_KEY_TERMINAL_LIGATURES, false))
      // 仅界面字体落到 --font-sans；编辑器/终端字体由各自组件读取 provider 状态后应用。
      document.documentElement.style.setProperty(
        "--font-sans",
        resolveFontStack(ui.id, ui.custom, "sans")
      )
    }

    const onStorage = (e: StorageEvent) => {
      if (e.key === STORAGE_KEY_THEME_COLOR && e.newValue) {
        const color = e.newValue as ThemeColor
        if ((THEME_COLORS as readonly string[]).includes(color)) {
          setThemeColorState(color)
          document.documentElement.setAttribute("data-theme", color)
        }
      }
      if (e.key === STORAGE_KEY_ZOOM_LEVEL && e.newValue) {
        const zoom = parseInt(e.newValue, 10) as ZoomLevel
        if ((ZOOM_LEVELS as readonly number[]).includes(zoom)) {
          setZoomLevelState(zoom)
          document.documentElement.style.fontSize = `${(16 * zoom) / 100}px`
          syncTrafficLightPosition(zoom)
        }
      }
      // "0" 是合法值，故不做 newValue 真值判断，交给 readBool 处理 null→默认。
      if (e.key === STORAGE_KEY_WELCOME_QUICK_ACTIONS) {
        setShowWelcomeQuickActionsState(
          readBool(STORAGE_KEY_WELCOME_QUICK_ACTIONS, true)
        )
      }
      if (e.key === STORAGE_KEY_MARKDOWN_PREVIEW_PRESERVE_LINE_BREAKS) {
        setMarkdownPreviewPreserveLineBreaksState(
          readBool(STORAGE_KEY_MARKDOWN_PREVIEW_PRESERVE_LINE_BREAKS, false)
        )
      }
      if (e.key && FONT_KEYS.has(e.key)) {
        rehydrateFonts()
      }
      // Sync appearance mode to Tauri DB when changed in another window
      if (e.key === "theme") {
        syncAppearanceMode(e.newValue ?? "system")
      }
    }
    window.addEventListener("storage", onStorage)
    return () => window.removeEventListener("storage", onStorage)
  }, [])

  return (
    <AppearanceContext.Provider
      value={{
        themeColor,
        setThemeColor,
        zoomLevel,
        setZoomLevel,
        showWelcomeQuickActions,
        setShowWelcomeQuickActions,
        markdownPreviewPreserveLineBreaks,
        setMarkdownPreviewPreserveLineBreaks,
        uiFont,
        setUiFont,
        editorFont,
        setEditorFont,
        terminalFont,
        setTerminalFont,
        editorFontSize,
        setEditorFontSize,
        terminalFontSize,
        setTerminalFontSize,
        editorLigatures,
        setEditorLigatures,
        editorWordWrap,
        setEditorWordWrap,
        terminalLigatures,
        setTerminalLigatures,
      }}
    >
      {children}
    </AppearanceContext.Provider>
  )
}
