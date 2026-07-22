"use client"

import { useEffect } from "react"
import { useRouter } from "next/navigation"
import { isDesktop } from "@/lib/platform"
import {
  clearCodegToken,
  consumeCodegTokenFromFragment,
  getCodegToken,
  setCodegToken,
} from "@/lib/transport/web-auth"

export default function Page() {
  const router = useRouter()
  useEffect(() => {
    if (isDesktop()) {
      router.replace("/workspace")
      return
    }
    // The local launcher passes its token in the URL fragment. Fragments never
    // reach the server; consume it once, scrub the address bar, then validate
    // it through the same health check as a manually entered token.
    const bootstrapToken = consumeCodegTokenFromFragment()
    if (bootstrapToken !== null) {
      setCodegToken(bootstrapToken)
    }

    // Web mode: validate token before entering app
    const token = bootstrapToken ?? getCodegToken()
    if (!token) {
      clearCodegToken()
      router.replace("/login")
      return
    }
    // Verify token is still valid
    fetch("/api/health", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
        Authorization: `Bearer ${token}`,
      },
      body: "{}",
    })
      .then((res) => {
        if (res.ok) {
          router.replace("/workspace")
          return
        }
        if (res.status === 401) {
          // Token genuinely rejected → clear it and re-authenticate.
          clearCodegToken()
          router.replace("/login")
          return
        }
        // Server reachable but unhealthy (5xx / proxy error). Keep the token
        // and enter the app; the in-app reconnect dialog handles recovery
        // instead of bouncing a valid session to /login.
        router.replace("/workspace")
      })
      .catch(() => {
        // Server unreachable (restart, network blip, sleep/wake). The token is
        // almost certainly still valid — don't discard it. Enter the workspace
        // and let WebConnectionGuard surface the offline state and recover.
        router.replace("/workspace")
      })
  }, [router])
  return null
}
