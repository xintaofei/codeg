"use client"

import { useEffect } from "react"
import { usePathname, useRouter } from "next/navigation"
import { useTranslations } from "next-intl"
import { isDesktop } from "@/lib/platform"

/**
 * Confirms before a same-document back/forward traversal leaves /workspace,
 * and before the page is closed or refreshed while the workspace is open.
 * In-window navigation (drawers, in-memory routes) is handled by
 * WorkspaceWindowHistoryProvider and never reaches this guard.
 * Applies on every platform; only the beforeunload half skips the desktop
 * client, where suppressing unload could block the app window from closing.
 */
export function WorkspaceLeaveGuard() {
  const t = useTranslations("Folder.workspaceContext")
  const pathname = usePathname()
  const router = useRouter()

  useEffect(() => {
    const onPopState = () => {
      if (
        pathname !== "/workspace" ||
        window.location.pathname === "/workspace"
      ) {
        return
      }
      if (!window.confirm(t("confirmLeaveWorkspace"))) {
        router.replace("/workspace", { scroll: false })
      }
    }
    window.addEventListener("popstate", onPopState)
    return () => window.removeEventListener("popstate", onPopState)
  }, [pathname, router, t])

  useEffect(() => {
    if (isDesktop() || pathname !== "/workspace") return
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault()
      event.returnValue = ""
    }
    window.addEventListener("beforeunload", onBeforeUnload)
    return () => window.removeEventListener("beforeunload", onBeforeUnload)
  }, [pathname])

  return null
}
