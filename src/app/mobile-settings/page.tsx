"use client"

import { useEffect, useState } from "react"
import { ArrowLeft, CheckCircle2, Loader2, LogOut, Server } from "lucide-react"
import { useRouter } from "next/navigation"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  getMobileServerUrl,
  normalizeServerUrl,
  setMobileServerUrl,
} from "@/lib/mobile-config"
import {
  clearCodegToken,
  getCodegToken,
  setCodegToken,
} from "@/lib/transport/web-auth"

export default function MobileSettingsPage() {
  const router = useRouter()
  const [serverUrl, setServerUrl] = useState("")
  const [token, setToken] = useState("")
  const [testing, setTesting] = useState(false)
  const [message, setMessage] = useState("")
  const [error, setError] = useState("")

  useEffect(() => {
    setServerUrl(getMobileServerUrl())
  }, [])

  const testAndSave = async () => {
    const normalized = normalizeServerUrl(serverUrl)
    const effectiveToken = token || getCodegToken()
    if (!normalized || !effectiveToken) {
      setError("请输入服务器地址；首次连接或更换凭据时还需输入 Token。")
      return
    }

    setTesting(true)
    setMessage("")
    setError("")
    try {
      const response = await fetch(`${normalized}/api/health`, {
        method: "POST",
        headers: {
          "Content-Type": "application/json",
          Authorization: `Bearer ${effectiveToken}`,
        },
        body: "{}",
      })
      if (!response.ok) {
        throw new Error(
          response.status === 401
            ? "Token 无效"
            : `服务器返回 HTTP ${response.status}`
        )
      }

      setMobileServerUrl(normalized)
      if (token) await setCodegToken(token)
      setMessage("连接测试成功，正在切换…")
      window.setTimeout(() => window.location.replace("/"), 250)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "连接失败")
    } finally {
      setTesting(false)
    }
  }

  const logOut = async () => {
    await clearCodegToken()
    window.location.replace("/login")
  }

  return (
    <main className="min-h-screen bg-background px-4 pb-[calc(24px+env(safe-area-inset-bottom))] pt-[calc(16px+env(safe-area-inset-top))] text-foreground">
      <div className="mx-auto max-w-lg space-y-6">
        <header className="flex min-h-12 items-center gap-3">
          <Button
            variant="ghost"
            size="icon"
            className="h-12 w-12 rounded-xl"
            onClick={() => router.back()}
            aria-label="返回"
          >
            <ArrowLeft className="h-5 w-5" />
          </Button>
          <div>
            <h1 className="text-lg font-semibold">移动连接</h1>
            <p className="text-xs text-muted-foreground">
              管理这台手机连接的 Codeg 服务器
            </p>
          </div>
        </header>

        <section className="space-y-5 rounded-2xl border bg-card p-4 shadow-sm">
          <div className="flex items-center gap-2">
            <Server className="h-5 w-5 text-primary" />
            <h2 className="font-medium">Direct HTTPS + WebSocket</h2>
          </div>

          <div className="space-y-2">
            <label htmlFor="mobile-server-url" className="text-sm font-medium">
              服务器地址
            </label>
            <Input
              id="mobile-server-url"
              type="url"
              inputMode="url"
              value={serverUrl}
              onChange={(event) => setServerUrl(event.target.value)}
              placeholder="https://codeg.example.com"
              autoCapitalize="none"
              autoCorrect="off"
              className="h-12 rounded-xl text-base"
            />
          </div>

          <div className="space-y-2">
            <label htmlFor="mobile-token" className="text-sm font-medium">
              新 Token（不修改可留空）
            </label>
            <Input
              id="mobile-token"
              type="password"
              value={token}
              onChange={(event) => setToken(event.target.value)}
              placeholder="保存在系统安全存储中"
              autoComplete="current-password"
              className="h-12 rounded-xl text-base"
            />
            <p className="text-xs leading-5 text-muted-foreground">
              Android 使用 Keystore，iOS 使用 Keychain。Token 不写入网页存储。
            </p>
          </div>

          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          {message && (
            <p className="flex items-center gap-2 text-sm text-emerald-500">
              <CheckCircle2 className="h-4 w-4" />
              {message}
            </p>
          )}

          <Button
            className="h-12 w-full rounded-xl text-base"
            onClick={() => void testAndSave()}
            disabled={testing || !serverUrl.trim()}
          >
            {testing && <Loader2 className="h-4 w-4 animate-spin" />}
            测试并保存
          </Button>
        </section>

        <Button
          variant="outline"
          className="h-12 w-full rounded-xl text-destructive"
          onClick={() => void logOut()}
        >
          <LogOut className="h-4 w-4" />
          退出登录并清除 Token
        </Button>
      </div>
    </main>
  )
}
