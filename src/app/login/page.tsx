"use client"

import { useEffect, useState } from "react"
import { useRouter } from "next/navigation"
import { useTranslations } from "next-intl"
import { isDesktop } from "@/lib/platform"
import { getMobileServerUrl, setMobileServerUrl } from "@/lib/mobile-config"
import { isMobileEnvironment } from "@/lib/transport/detect"
import { setCodegToken } from "@/lib/transport/web-auth"

export default function LoginPage() {
  const router = useRouter()
  const t = useTranslations("LoginPage")
  const [token, setToken] = useState("")
  const [serverUrl, setServerUrl] = useState("")
  const [error, setError] = useState("")
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    document.title = t("documentTitle")
    if (isMobileEnvironment()) {
      setServerUrl(getMobileServerUrl())
    }
  }, [t])

  // Desktop users skip login entirely
  if (isDesktop()) {
    router.replace("/workspace")
    return null
  }

  async function handleSubmit(e: React.FormEvent) {
    e.preventDefault()
    setError("")
    setLoading(true)

    try {
      const mobile = isMobileEnvironment()
      const normalizedServerUrl = mobile ? setMobileServerUrl(serverUrl) : ""
      if (mobile && !normalizedServerUrl) {
        setError("请输入 Codeg 服务器地址")
        return
      }
      // Validate token by calling a lightweight API endpoint
      const res = await fetch(`${normalizedServerUrl}/api/health`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${token}`,
        },
        body: "{}",
      })

      if (res.ok) {
        await setCodegToken(token)
        router.replace("/workspace")
      } else if (res.status === 401) {
        setError(t("invalidToken"))
      } else {
        setError(t("connectionFailed", { status: res.status }))
      }
    } catch {
      setError(t("networkError"))
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-background">
      <div className="w-full max-w-sm space-y-6 px-4">
        <div className="space-y-2 text-center">
          <h1 className="text-2xl font-bold tracking-tight">{t("brand")}</h1>
          <p className="text-sm text-muted-foreground">{t("subtitle")}</p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          {isMobileEnvironment() && (
            <div className="space-y-2">
              <label
                htmlFor="server-url"
                className="text-sm font-medium text-foreground"
              >
                Codeg 服务器
              </label>
              <input
                id="server-url"
                type="url"
                inputMode="url"
                value={serverUrl}
                onChange={(e) => setServerUrl(e.target.value)}
                placeholder="https://codeg.example.com"
                autoCapitalize="none"
                autoCorrect="off"
                className="flex h-12 w-full rounded-xl border border-input bg-background px-4 py-2 text-base ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
              />
              <p className="text-xs text-muted-foreground">
                当前版本使用 HTTPS + WebSocket 直连，Agent 仍在电脑运行。
              </p>
            </div>
          )}
          <div className="space-y-2">
            {isMobileEnvironment() && (
              <label
                htmlFor="access-token"
                className="text-sm font-medium text-foreground"
              >
                访问 Token
              </label>
            )}
            <input
              id="access-token"
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder={t("tokenPlaceholder")}
              autoFocus
              className="flex h-12 w-full rounded-xl border border-input bg-background px-4 py-2 text-base ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            />
          </div>

          {error && <p className="text-sm text-destructive">{error}</p>}

          <button
            type="submit"
            disabled={
              !token || (isMobileEnvironment() && !serverUrl) || loading
            }
            className="inline-flex h-12 w-full items-center justify-center rounded-xl bg-primary px-4 py-2 text-base font-medium text-primary-foreground ring-offset-background transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50"
          >
            {loading ? t("connecting") : t("connect")}
          </button>
        </form>

        <p className="text-center text-xs text-muted-foreground">
          {t("helpText")}
        </p>
      </div>
    </div>
  )
}
