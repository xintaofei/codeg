"use client"

import { useEffect, useState } from "react"

import { detectEnvironment, isMobileEnvironment } from "@/lib/transport/detect"
import { bootstrapCodegToken } from "@/lib/transport/web-auth"
import {
  bootstrapMobileRelaySecrets,
  getMobileRelayConfig,
} from "@/lib/relay/config"
import { getShellTransport } from "@/lib/transport"

export function MobileAuthBootstrap({
  children,
}: {
  children: React.ReactNode
}) {
  const [ready, setReady] = useState(false)

  useEffect(() => {
    let active = true
    const bootstrap = async () => {
      if (isMobileEnvironment()) {
        try {
          const relayBootstrap = bootstrapMobileRelaySecrets().then(() => {
            // Start DNS/TLS/WebSocket negotiation as soon as KeyStore has
            // released the Relay secrets. Workspace providers mount next and
            // reuse this singleton, saving a full render before the handshake.
            if (
              detectEnvironment() === "mobile-relay" &&
              getMobileRelayConfig()
            ) {
              getShellTransport()
            }
          })
          await Promise.all([bootstrapCodegToken(), relayBootstrap])
        } catch (error) {
          console.error("[Mobile] secure token bootstrap failed", error)
        }
      }
      if (active) setReady(true)
    }
    void bootstrap()
    return () => {
      active = false
    }
  }, [])

  if (!ready) {
    return (
      <div className="flex min-h-screen items-center justify-center bg-background text-foreground">
        <div className="text-center">
          <div className="text-xl font-bold tracking-tight">codeg</div>
          <div className="mt-2 text-xs text-muted-foreground">
            正在安全加载连接…
          </div>
        </div>
      </div>
    )
  }

  return children
}
