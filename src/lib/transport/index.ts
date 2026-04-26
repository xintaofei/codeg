import { detectEnvironment } from "./detect"
import type { Transport } from "./types"

export type { Transport, UnsubscribeFn } from "./types"

let _transport: Transport | null = null

export function getTransport(): Transport {
  if (!_transport) {
    const env = detectEnvironment()
    if (env === "tauri") {
      // Use dynamic require to avoid bundling tauri deps in web mode.
      // eslint-disable-next-line @typescript-eslint/no-require-imports
      const { TauriTransport } = require("./tauri-transport") as {
        TauriTransport: new () => Transport
      }
      _transport = new TauriTransport()
    } else {
      // eslint-disable-next-line @typescript-eslint/no-require-imports
      const { WebTransport } = require("./web-transport") as {
        WebTransport: new (baseUrl: string) => Transport
      }
      const baseUrl = window.location.origin
      _transport = new WebTransport(baseUrl)
    }
  }
  return _transport
}

export function isDesktop(): boolean {
  return detectEnvironment() === "tauri"
}
