"use client"

import { useEffect, useState } from "react"
import {
  ArrowLeft,
  CheckCircle2,
  Loader2,
  LogOut,
  RadioTower,
  Server,
} from "lucide-react"
import { useRouter } from "next/navigation"

import { Button } from "@/components/ui/button"
import { Input } from "@/components/ui/input"
import {
  clearMobileServer,
  getMobileConnectionMode,
  getMobileServerUrl,
  normalizeServerUrl,
  setMobileConnectionMode,
  setMobileServerUrl,
  type MobileConnectionMode,
} from "@/lib/mobile-config"
import {
  clearMobileRelayConfig,
  getMobileRelayConfig,
  getMobileRelayPublicConfig,
  parseMobileRelayPairingPayload,
  setMobileRelayConfig,
} from "@/lib/relay/config"
import {
  clearCodegToken,
  getCodegToken,
  setCodegToken,
} from "@/lib/transport/web-auth"
import { RelayQrScanner } from "@/components/mobile/relay-qr-scanner"

export default function MobileSettingsPage() {
  const router = useRouter()
  const [serverUrl, setServerUrl] = useState("")
  const [mode, setMode] = useState<MobileConnectionMode>("direct")
  const [pairingPayload, setPairingPayload] = useState("")
  const [relaySummary, setRelaySummary] = useState("")
  const [token, setToken] = useState("")
  const [testing, setTesting] = useState(false)
  const [message, setMessage] = useState("")
  const [error, setError] = useState("")

  useEffect(() => {
    setServerUrl(getMobileServerUrl())
    setMode(getMobileConnectionMode())
    const relay = getMobileRelayPublicConfig()
    if (relay.relayUrl) {
      setRelaySummary(`${relay.deviceId} · ${relay.relayUrl}`)
    }
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
      setMobileConnectionMode("direct")
      if (token) await setCodegToken(token)
      setMessage("连接测试成功，正在切换…")
      window.setTimeout(() => window.location.replace("/"), 250)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "连接失败")
    } finally {
      setTesting(false)
    }
  }

  const pairRelay = async () => {
    setTesting(true)
    setMessage("")
    setError("")
    try {
      if (pairingPayload.trim()) {
        const config = parseMobileRelayPairingPayload(pairingPayload)
        await setMobileRelayConfig(config)
      } else if (!getMobileRelayConfig()) {
        throw new Error("请扫描二维码或粘贴电脑显示的配对内容。")
      }
      setMobileConnectionMode("relay")
      setMessage("Relay 凭据已安全保存，正在切换…")
      window.setTimeout(() => window.location.replace("/"), 250)
    } catch (cause) {
      setError(cause instanceof Error ? cause.message : "Relay 配对失败")
    } finally {
      setTesting(false)
    }
  }

  const logOut = async () => {
    await Promise.all([clearCodegToken(), clearMobileRelayConfig()])
    clearMobileServer()
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
          <div className="grid grid-cols-2 gap-2 rounded-xl bg-muted p-1">
            <button
              type="button"
              className={`h-11 rounded-lg text-sm font-medium transition-colors ${
                mode === "direct"
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground"
              }`}
              onClick={() => {
                setMode("direct")
                setError("")
                setMessage("")
              }}
            >
              Direct
            </button>
            <button
              type="button"
              className={`h-11 rounded-lg text-sm font-medium transition-colors ${
                mode === "relay"
                  ? "bg-background text-foreground shadow-sm"
                  : "text-muted-foreground"
              }`}
              onClick={() => {
                setMode("relay")
                setError("")
                setMessage("")
              }}
            >
              Relay
            </button>
          </div>

          {mode === "direct" ? (
            <>
              <div className="flex items-center gap-2">
                <Server className="h-5 w-5 text-primary" />
                <h2 className="font-medium">Direct HTTPS + WebSocket</h2>
              </div>

              <div className="space-y-2">
                <label
                  htmlFor="mobile-server-url"
                  className="text-sm font-medium"
                >
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
                  Android 使用 Keystore，iOS 使用 Keychain。Token
                  不写入网页存储。
                </p>
              </div>

              <Button
                className="h-12 w-full rounded-xl text-base"
                onClick={() => void testAndSave()}
                disabled={testing || !serverUrl.trim()}
              >
                {testing && <Loader2 className="h-4 w-4 animate-spin" />}
                测试并保存
              </Button>
            </>
          ) : (
            <>
              <div className="flex items-center gap-2">
                <RadioTower className="h-5 w-5 text-primary" />
                <div>
                  <h2 className="font-medium">Relay 端到端加密</h2>
                  <p className="text-xs text-muted-foreground">
                    无需电脑公网 IP 或路由器端口映射
                  </p>
                </div>
              </div>

              {relaySummary && (
                <div className="rounded-xl border bg-muted/40 px-3 py-2 text-xs text-muted-foreground">
                  已配对：{relaySummary}
                </div>
              )}

              <div className="space-y-2">
                <label
                  htmlFor="relay-pairing-payload"
                  className="text-sm font-medium"
                >
                  新配对内容（沿用当前设备可留空）
                </label>
                <textarea
                  id="relay-pairing-payload"
                  value={pairingPayload}
                  onChange={(event) => setPairingPayload(event.target.value)}
                  placeholder="扫描二维码，或粘贴电脑显示的一次性配对内容"
                  autoCapitalize="none"
                  autoCorrect="off"
                  rows={6}
                  className="flex w-full resize-none rounded-xl border border-input bg-background px-4 py-3 text-base ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                />
                <RelayQrScanner
                  onDetected={(payload) => {
                    setPairingPayload(payload)
                    setError("")
                  }}
                />
                <p className="text-xs leading-5 text-muted-foreground">
                  Relay 看不到 Codeg
                  Token、聊天、代码和命令正文；配对根密钥只保存在系统安全存储。
                </p>
              </div>

              <Button
                className="h-12 w-full rounded-xl text-base"
                onClick={() => void pairRelay()}
                disabled={testing || (!pairingPayload.trim() && !relaySummary)}
              >
                {testing && <Loader2 className="h-4 w-4 animate-spin" />}
                {relaySummary && !pairingPayload.trim()
                  ? "使用已配对 Relay"
                  : "安全配对并切换"}
              </Button>
            </>
          )}

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
