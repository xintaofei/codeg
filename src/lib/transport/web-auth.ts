// Shared helpers for web-mode HTTP calls — the JSON transport in
// `web-transport.ts` and direct multipart/file callers in `lib/api.ts` both
// need consistent token retrieval and 401 redirect behavior. Keeping them in
// one place means a future move from `localStorage` to cookies (or rotation
// rules, multi-tenant prefixing, etc.) doesn't have to be remembered at every
// call site.

const TOKEN_KEY = "codeg_token"
const TOKEN_FRAGMENT_KEY = "codeg_token"

export function getCodegToken(): string {
  return localStorage.getItem(TOKEN_KEY) ?? ""
}

export function setCodegToken(token: string): void {
  localStorage.setItem(TOKEN_KEY, token)
}

export function clearCodegToken(): void {
  localStorage.removeItem(TOKEN_KEY)
}

export function consumeCodegTokenFromFragment(): string | null {
  const rawFragment = window.location.hash.slice(1)
  if (!rawFragment) return null

  const params = new URLSearchParams(rawFragment)
  const token = params.get(TOKEN_FRAGMENT_KEY)
  if (token === null) return null

  params.delete(TOKEN_FRAGMENT_KEY)
  const remainingFragment = params.toString()
  const cleanUrl = `${window.location.pathname}${window.location.search}${
    remainingFragment ? `#${remainingFragment}` : ""
  }`
  window.history.replaceState(window.history.state, "", cleanUrl)
  return token
}

export function redirectToCodegLogin(): void {
  if (window.location.pathname.startsWith("/login")) return
  clearCodegToken()
  window.location.href = "/login"
}
