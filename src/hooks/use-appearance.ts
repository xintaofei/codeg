"use client"

import { useContext } from "react"
import { AppearanceContext } from "@/components/appearance-provider"

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

/** 语义化包装：只关心 UI 字体的调用点用这个 */
export function useUiFontFamily() {
  const { uiFontFamily, setUiFontFamily, uiFontFamilyStack } = useAppearance()
  return { uiFontFamily, setUiFontFamily, uiFontFamilyStack }
}

/** 语义化包装：只关心代码字体的调用点用这个 */
export function useCodeFontFamily() {
  const { codeFontFamily, setCodeFontFamily, codeFontFamilyStack } =
    useAppearance()
  return { codeFontFamily, setCodeFontFamily, codeFontFamilyStack }
}
