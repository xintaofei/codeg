"use client"

import { useEffect } from "react"
import type { ThemeProviderProps } from "next-themes"
import { ThemeProvider as NextThemesProvider, useTheme } from "next-themes"

function ThemeColorMetaSync() {
  const { resolvedTheme, theme } = useTheme()

  useEffect(() => {
    const isDark = resolvedTheme === "dark"
    const metaTags = document.querySelectorAll('meta[name="theme-color"]')
    if (!metaTags.length) return

    if (theme === "dark" || theme === "light") {
      metaTags.forEach((tag) => {
        tag.setAttribute("content", isDark ? "#09090b" : "#ffffff")
      })
    } else {
      metaTags.forEach((tag) => {
        const media = tag.getAttribute("media")
        if (media && media.includes("dark")) {
          tag.setAttribute("content", "#09090b")
        } else if (media && media.includes("light")) {
          tag.setAttribute("content", "#ffffff")
        }
      })
    }
  }, [resolvedTheme, theme])

  return null
}

export function ThemeProvider({ children, ...props }: ThemeProviderProps) {
  return (
    <NextThemesProvider {...props}>
      <ThemeColorMetaSync />
      {children}
    </NextThemesProvider>
  )
}
