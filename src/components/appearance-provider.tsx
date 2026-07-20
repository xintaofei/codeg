"use client"

import { createContext, useCallback, useEffect, useRef, useState } from "react"
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
  STORAGE_KEY_WORKSPACE_BG_ENABLED,
  STORAGE_KEY_WORKSPACE_BG_MASK,
  STORAGE_KEY_WORKSPACE_BG_BLUR,
  STORAGE_KEY_WORKSPACE_BG_FILL,
  STORAGE_KEY_WORKSPACE_BG_PANEL_OPACITY,
  STORAGE_KEY_WORKSPACE_BG_IMAGE_VERSION,
} from "@/lib/appearance-script"
import {
  DEFAULT_WORKSPACE_BG_ENABLED,
  DEFAULT_WORKSPACE_BG_MASK_OPACITY,
  DEFAULT_WORKSPACE_BG_IMAGE_BLUR,
  DEFAULT_WORKSPACE_BG_PANEL_OPACITY,
  DEFAULT_WORKSPACE_BG_FILL_MODE,
  clampMaskOpacity,
  clampImageBlur,
  clampPanelOpacity,
  isValidFillMode,
  createBackgroundObjectUrl,
  revokeBackgroundObjectUrl,
  readWorkspaceBackground,
  setWorkspaceBackground,
  clearWorkspaceBackground,
  type WorkspaceBgFillMode,
} from "@/lib/workspace-background"

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
  /** Workspace 背景图片总开关。关闭时不加载图片、不触发任何表面半透明。 */
  workspaceBgEnabled: boolean
  setWorkspaceBgEnabled: (on: boolean) => void
  /** 暗化遮罩不透明度（朝 --background 的面纱，明暗自适配），0–0.9。 */
  workspaceBgMaskOpacity: number
  setWorkspaceBgMaskOpacity: (v: number) => void
  /** 背景图片模糊半径（px），0–24。 */
  workspaceBgImageBlur: number
  setWorkspaceBgImageBlur: (v: number) => void
  /** 结构性面板（侧栏/面板/标签条）不透明度，驱动 --ws-surface-alpha，0.3–1。 */
  workspaceBgPanelOpacity: number
  setWorkspaceBgPanelOpacity: (v: number) => void
  /** 背景图片填充模式（cover/contain/center/tile）。 */
  workspaceBgFillMode: WorkspaceBgFillMode
  setWorkspaceBgFillMode: (mode: WorkspaceBgFillMode) => void
  /** 已解析的背景图片 blob URL（异步从磁盘加载），无图为 null。 */
  workspaceBgImageUrl: string | null
  /** 上传并设置背景图片（base64）。写盘后重新读回并建 blob URL。 */
  setWorkspaceBackgroundImage: (imageBase64: string) => Promise<void>
  /** 移除背景图片（删盘 + revoke blob URL）。 */
  removeWorkspaceBackground: () => Promise<void>
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

function readNumber(
  key: string,
  def: number,
  clampFn: (v: number) => number
): number {
  if (typeof document === "undefined") return def
  try {
    const raw = localStorage.getItem(key)
    if (raw === null) return def
    const n = parseFloat(raw)
    return Number.isNaN(n) ? def : clampFn(n)
  } catch {
    return def
  }
}

function readWorkspaceBgFillMode(): WorkspaceBgFillMode {
  if (typeof document === "undefined") return DEFAULT_WORKSPACE_BG_FILL_MODE
  try {
    const raw = localStorage.getItem(STORAGE_KEY_WORKSPACE_BG_FILL)
    return isValidFillMode(raw) ? raw : DEFAULT_WORKSPACE_BG_FILL_MODE
  } catch {
    return DEFAULT_WORKSPACE_BG_FILL_MODE
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

  // Workspace 背景图片配置（图片 URL 异步加载，初始 null）。
  const [workspaceBgEnabled, setWorkspaceBgEnabledState] = useState<boolean>(
    () =>
      readBool(STORAGE_KEY_WORKSPACE_BG_ENABLED, DEFAULT_WORKSPACE_BG_ENABLED)
  )
  const [workspaceBgMaskOpacity, setWorkspaceBgMaskOpacityState] =
    useState<number>(() =>
      readNumber(
        STORAGE_KEY_WORKSPACE_BG_MASK,
        DEFAULT_WORKSPACE_BG_MASK_OPACITY,
        clampMaskOpacity
      )
    )
  const [workspaceBgImageBlur, setWorkspaceBgImageBlurState] = useState<number>(
    () =>
      readNumber(
        STORAGE_KEY_WORKSPACE_BG_BLUR,
        DEFAULT_WORKSPACE_BG_IMAGE_BLUR,
        clampImageBlur
      )
  )
  const [workspaceBgPanelOpacity, setWorkspaceBgPanelOpacityState] =
    useState<number>(() =>
      readNumber(
        STORAGE_KEY_WORKSPACE_BG_PANEL_OPACITY,
        DEFAULT_WORKSPACE_BG_PANEL_OPACITY,
        clampPanelOpacity
      )
    )
  const [workspaceBgFillMode, setWorkspaceBgFillModeState] =
    useState<WorkspaceBgFillMode>(() => readWorkspaceBgFillMode())
  const [workspaceBgImageUrl, setWorkspaceBgImageUrlState] = useState<
    string | null
  >(null)

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

  // enabled 与 panelOpacity 的 DOM 应用（data-workspace-bg 属性 + --ws-surface-alpha）
  // 统一交给下方一个 effect，覆盖 mount、重启后 re-enable、跨标签所有路径。setter 只
  // 更新 state + 持久化，避免 --ws-surface-alpha 与 state 失同步（否则重启后 re-enable
  // 会沿用默认值而非用户设定值）。
  const setWorkspaceBgEnabled = useCallback((on: boolean) => {
    setWorkspaceBgEnabledState(on)
    persist(STORAGE_KEY_WORKSPACE_BG_ENABLED, on ? "1" : "0")
  }, [])

  const setWorkspaceBgMaskOpacity = useCallback((v: number) => {
    const clamped = clampMaskOpacity(v)
    setWorkspaceBgMaskOpacityState(clamped)
    persist(STORAGE_KEY_WORKSPACE_BG_MASK, String(clamped))
  }, [])

  const setWorkspaceBgImageBlur = useCallback((v: number) => {
    const clamped = clampImageBlur(v)
    setWorkspaceBgImageBlurState(clamped)
    persist(STORAGE_KEY_WORKSPACE_BG_BLUR, String(clamped))
  }, [])

  const setWorkspaceBgPanelOpacity = useCallback((v: number) => {
    const clamped = clampPanelOpacity(v)
    setWorkspaceBgPanelOpacityState(clamped)
    persist(STORAGE_KEY_WORKSPACE_BG_PANEL_OPACITY, String(clamped))
  }, [])

  const setWorkspaceBgFillMode = useCallback((mode: WorkspaceBgFillMode) => {
    setWorkspaceBgFillModeState(mode)
    persist(STORAGE_KEY_WORKSPACE_BG_FILL, mode)
  }, [])

  // 并发 reload 的代次守卫：只有最新一次请求的读结果被应用。避免旧读在更晚的
  // 写/清空之后完成、把状态回退到过期图（re-enable 与 setImage/remove、或多次快速
  // 切换的竞态）。写入窗口收不到自己的 storage 事件，本地一致性全靠这个守卫。
  const reloadGenRef = useRef(0)

  // 从磁盘重新读取背景图并刷新 blob URL（revoke 旧、建新或置 null）。写/换/删图
  // 与跨窗口版本戳变更都复用它，确保 URL 生命周期与磁盘状态一致。
  const reloadWorkspaceBackgroundImage = useCallback(async () => {
    const gen = ++reloadGenRef.current
    try {
      const asset = await readWorkspaceBackground()
      // 期间有更新的请求（写/清空/更晚的 reload）→ 丢弃本次过期结果，也不建 blob。
      if (gen !== reloadGenRef.current) return
      setWorkspaceBgImageUrlState((prev) => {
        revokeBackgroundObjectUrl(prev)
        return asset ? createBackgroundObjectUrl(asset) : null
      })
    } catch {
      // 读盘失败静默（无背景即可）。
    }
  }, [])

  const setWorkspaceBackgroundImage = useCallback(
    async (imageBase64: string) => {
      await setWorkspaceBackground(imageBase64)
      // 写盘持久化后立即广播版本戳（不等本地 readback）：避免设置窗口在读回大图
      // 期间被关闭，导致 workspace 窗口收不到失效信号、停留在旧图。随后再刷新本地预览。
      persist(STORAGE_KEY_WORKSPACE_BG_IMAGE_VERSION, String(Date.now()))
      await reloadWorkspaceBackgroundImage()
    },
    [reloadWorkspaceBackgroundImage]
  )

  const removeWorkspaceBackground = useCallback(async () => {
    await clearWorkspaceBackground()
    // 使任何在途 reload 失效（否则先前发起的旧读可能在清空后完成、恢复已删的图），
    // 立即广播失效戳，再置空本地预览。
    reloadGenRef.current += 1
    persist(STORAGE_KEY_WORKSPACE_BG_IMAGE_VERSION, String(Date.now()))
    setWorkspaceBgImageUrlState((prev) => {
      revokeBackgroundObjectUrl(prev)
      return null
    })
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

  // enabled + panelOpacity 的 DOM 单一同步点：属性驱动 globals.css 的半透明规则，
  // CSS 变量驱动面板不透明度。覆盖 mount / 重启后 re-enable / setter / 跨标签，确保
  // DOM 始终与 state 一致（inline 脚本只在首帧 enabled 时预置，之后由此接管）。
  useEffect(() => {
    document.documentElement.setAttribute(
      "data-workspace-bg",
      workspaceBgEnabled ? "on" : "off"
    )
    if (workspaceBgEnabled) {
      document.documentElement.style.setProperty(
        "--ws-surface-alpha",
        String(workspaceBgPanelOpacity)
      )
    }
  }, [workspaceBgEnabled, workspaceBgPanelOpacity])

  // 背景图片异步从磁盘加载：仅在启用时拉取，未启用零开销。图片较大，晚到几十~
  // 几百 ms 只是装饰层淡入（底色是 --background），可接受。写/换/删与跨窗口同步复用
  // reload helper。
  useEffect(() => {
    if (!workspaceBgEnabled) return
    void reloadWorkspaceBackgroundImage()
  }, [workspaceBgEnabled, reloadWorkspaceBackgroundImage])

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
      if (e.key && FONT_KEYS.has(e.key)) {
        rehydrateFonts()
      }
      // Workspace 背景配置跨标签页同步。enabled/panel-opacity 需同步 DOM
      // （属性 + CSS 变量），mask/blur/fill 仅同步 state（React 层消费）。
      // enabled/panel-opacity 只更新 state；DOM（属性 + --ws-surface-alpha）由上方
      // 统一 effect 跟随 state 同步。
      if (e.key === STORAGE_KEY_WORKSPACE_BG_ENABLED) {
        setWorkspaceBgEnabledState(
          readBool(
            STORAGE_KEY_WORKSPACE_BG_ENABLED,
            DEFAULT_WORKSPACE_BG_ENABLED
          )
        )
      }
      if (e.key === STORAGE_KEY_WORKSPACE_BG_PANEL_OPACITY) {
        setWorkspaceBgPanelOpacityState(
          readNumber(
            STORAGE_KEY_WORKSPACE_BG_PANEL_OPACITY,
            DEFAULT_WORKSPACE_BG_PANEL_OPACITY,
            clampPanelOpacity
          )
        )
      }
      if (e.key === STORAGE_KEY_WORKSPACE_BG_MASK) {
        setWorkspaceBgMaskOpacityState(
          readNumber(
            STORAGE_KEY_WORKSPACE_BG_MASK,
            DEFAULT_WORKSPACE_BG_MASK_OPACITY,
            clampMaskOpacity
          )
        )
      }
      if (e.key === STORAGE_KEY_WORKSPACE_BG_BLUR) {
        setWorkspaceBgImageBlurState(
          readNumber(
            STORAGE_KEY_WORKSPACE_BG_BLUR,
            DEFAULT_WORKSPACE_BG_IMAGE_BLUR,
            clampImageBlur
          )
        )
      }
      if (e.key === STORAGE_KEY_WORKSPACE_BG_FILL) {
        setWorkspaceBgFillModeState(readWorkspaceBgFillMode())
      }
      // 图片版本戳变化（另一窗口写/换/删图）：重新读盘刷新本窗口 blob URL。
      if (e.key === STORAGE_KEY_WORKSPACE_BG_IMAGE_VERSION) {
        void reloadWorkspaceBackgroundImage()
      }
      // Sync appearance mode to Tauri DB when changed in another window
      if (e.key === "theme") {
        syncAppearanceMode(e.newValue ?? "system")
      }
    }
    window.addEventListener("storage", onStorage)
    return () => window.removeEventListener("storage", onStorage)
  }, [reloadWorkspaceBackgroundImage])

  return (
    <AppearanceContext.Provider
      value={{
        themeColor,
        setThemeColor,
        zoomLevel,
        setZoomLevel,
        showWelcomeQuickActions,
        setShowWelcomeQuickActions,
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
        workspaceBgEnabled,
        setWorkspaceBgEnabled,
        workspaceBgMaskOpacity,
        setWorkspaceBgMaskOpacity,
        workspaceBgImageBlur,
        setWorkspaceBgImageBlur,
        workspaceBgPanelOpacity,
        setWorkspaceBgPanelOpacity,
        workspaceBgFillMode,
        setWorkspaceBgFillMode,
        workspaceBgImageUrl,
        setWorkspaceBackgroundImage,
        removeWorkspaceBackground,
      }}
    >
      {children}
    </AppearanceContext.Provider>
  )
}
