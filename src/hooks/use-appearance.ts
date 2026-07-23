"use client"

import { useContext } from "react"
import { AppearanceContext } from "@/components/appearance-provider"
import { resolveFontStack } from "@/lib/font-presets"

export function useAppearance() {
  const ctx = useContext(AppearanceContext)
  if (!ctx) {
    throw new Error("useAppearance must be used within AppearanceProvider")
  }
  return ctx
}

/** 语义化包装：只关心主题色的调用点用这个 */
export function useThemeColor() {
  const { themeColor, setThemeColor } = useAppearance()
  return { themeColor, setThemeColor }
}

/** 语义化包装：只关心缩放档位的调用点用这个 */
export function useZoomLevel() {
  const { zoomLevel, setZoomLevel } = useAppearance()
  return { zoomLevel, setZoomLevel }
}

/** 语义化包装：新会话欢迎页「模式选择区域」显示开关 */
export function useWelcomeQuickActions() {
  const { showWelcomeQuickActions, setShowWelcomeQuickActions } =
    useAppearance()
  return { showWelcomeQuickActions, setShowWelcomeQuickActions }
}

/** 界面字体（普通组件）。stack 已解析，可直接用于 style 或 CSS 变量。 */
export function useUiFont() {
  const { uiFont, setUiFont } = useAppearance()
  return {
    uiFont,
    setUiFont,
    uiFontStack: resolveFontStack(uiFont.id, uiFont.custom, "sans"),
  }
}

/** 编辑器字体（Monaco）：含字号、连字与自动换行。stack 已解析。 */
export function useEditorFont() {
  const {
    editorFont,
    setEditorFont,
    editorFontSize,
    setEditorFontSize,
    editorLigatures,
    setEditorLigatures,
    editorWordWrap,
    setEditorWordWrap,
  } = useAppearance()
  return {
    editorFont,
    setEditorFont,
    editorFontStack: resolveFontStack(editorFont.id, editorFont.custom, "mono"),
    editorFontSize,
    setEditorFontSize,
    editorLigatures,
    setEditorLigatures,
    editorWordWrap,
    setEditorWordWrap,
  }
}

/** 终端字体（xterm）：含字号与连字。stack 已解析。 */
export function useTerminalFont() {
  const {
    terminalFont,
    setTerminalFont,
    terminalFontSize,
    setTerminalFontSize,
    terminalLigatures,
    setTerminalLigatures,
  } = useAppearance()
  return {
    terminalFont,
    setTerminalFont,
    terminalFontStack: resolveFontStack(
      terminalFont.id,
      terminalFont.custom,
      "mono"
    ),
    terminalFontSize,
    setTerminalFontSize,
    terminalLigatures,
    setTerminalLigatures,
  }
}

/** Workspace 背景图片：启用开关、遮罩/模糊/填充/面板不透明度配置、以及已解析的
 * 图片 blob URL（异步从磁盘加载）。供外观设置面板与 workspace 布局共同消费。 */
export function useWorkspaceBackground() {
  const {
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
  } = useAppearance()
  return {
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
  }
}
