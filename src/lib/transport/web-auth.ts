// Shared helpers for web-mode HTTP calls — the JSON transport in
// `web-transport.ts` and direct multipart/file callers in `lib/api.ts` both
// need consistent token retrieval and 401 redirect behavior. Keeping them in
// one place means a future move from `localStorage` to cookies (or rotation
// rules, multi-tenant prefixing, etc.) doesn't have to be remembered at every
// call site.

const TOKEN_KEY = "codeg_token"
let mobileToken = ""

function isMobileTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    "__TAURI_INTERNALS__" in window &&
    /Android|iPhone|iPad|iPod/i.test(navigator.userAgent)
  )
}

export function getCodegToken(): string {
  if (isMobileTauri()) return mobileToken
  return localStorage.getItem(TOKEN_KEY) ?? ""
}

export async function bootstrapCodegToken(): Promise<void> {
  if (!isMobileTauri()) return
  const { invoke } = await import("@tauri-apps/api/core")
  const result = await invoke<{ value?: string }>(
    "plugin:secure-vault|load_secret",
    { payload: { key: TOKEN_KEY } }
  )
  mobileToken = result.value ?? ""
  // Remove tokens left by pre-Keystore development builds.
  localStorage.removeItem(TOKEN_KEY)
}

export async function setCodegToken(token: string): Promise<void> {
  if (!isMobileTauri()) {
    localStorage.setItem(TOKEN_KEY, token)
    return
  }
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("plugin:secure-vault|store_secret", {
    payload: { key: TOKEN_KEY, value: token },
  })
  mobileToken = token
  localStorage.removeItem(TOKEN_KEY)
}

export async function clearCodegToken(): Promise<void> {
  mobileToken = ""
  localStorage.removeItem(TOKEN_KEY)
  if (!isMobileTauri()) return
  const { invoke } = await import("@tauri-apps/api/core")
  await invoke("plugin:secure-vault|delete_secret", {
    payload: { key: TOKEN_KEY },
  })
}

export async function redirectToCodegLogin(): Promise<void> {
  if (window.location.pathname.startsWith("/login")) return
  await clearCodegToken()
  window.location.href = "/login"
}
