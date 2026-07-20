export type TransportEnvironment =
  | "desktop-local"
  | "browser-remote"
  | "mobile-direct"
  | "mobile-relay"

function isMobileUserAgent(): boolean {
  if (typeof navigator === "undefined") return false
  return /Android|iPhone|iPad|iPod/i.test(navigator.userAgent)
}

export function detectEnvironment(): TransportEnvironment {
  if (typeof window !== "undefined" && "__TAURI_INTERNALS__" in window) {
    if (isMobileUserAgent()) {
      const mode = localStorage.getItem("codeg_mobile_connection_mode")
      return mode === "relay" ? "mobile-relay" : "mobile-direct"
    }
    return "desktop-local"
  }
  return "browser-remote"
}

export function isMobileEnvironment(): boolean {
  const environment = detectEnvironment()
  return environment === "mobile-direct" || environment === "mobile-relay"
}
