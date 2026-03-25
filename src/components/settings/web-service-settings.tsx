"use client"

import { useCallback, useEffect, useState } from "react"
import { Check, Copy, ExternalLink, Eye, EyeOff } from "lucide-react"
import {
  startWebServer,
  stopWebServer,
  getWebServerStatus,
  type WebServerInfo,
} from "@/lib/api"
import { openUrl } from "@/lib/platform"

function AddressCard({ label, value }: { label: string; value: string }) {
  const [hovered, setHovered] = useState(false)

  return (
    <div className="space-y-1.5">
      <div className="text-xs font-medium text-muted-foreground">{label}</div>
      <div
        className="group relative flex items-center rounded-md border bg-muted/40 px-3 py-2"
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      >
        <code className="min-w-0 flex-1 truncate text-sm select-all">
          {value}
        </code>
        <div
          className={`ml-2 flex shrink-0 items-center gap-1 transition-opacity ${
            hovered ? "opacity-100" : "opacity-0"
          }`}
        >
          <button
            type="button"
            onClick={() => openUrl(value)}
            className="inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground"
            title="打开"
          >
            <ExternalLink className="h-3.5 w-3.5" />
          </button>
        </div>
      </div>
    </div>
  )
}

function TokenCard({ label, value }: { label: string; value: string }) {
  const [hovered, setHovered] = useState(false)
  const [copied, setCopied] = useState(false)
  const [revealed, setRevealed] = useState(false)

  function handleCopy() {
    navigator.clipboard.writeText(value)
    setCopied(true)
    setTimeout(() => setCopied(false), 1500)
  }

  const displayValue = revealed
    ? value
    : "\u2022".repeat(Math.max(value.length, 12))

  return (
    <div className="space-y-1.5">
      <div className="text-xs font-medium text-muted-foreground">{label}</div>
      <div
        className="group relative flex items-center rounded-md border bg-muted/40 px-3 py-2"
        onMouseEnter={() => setHovered(true)}
        onMouseLeave={() => setHovered(false)}
      >
        <code className="min-w-0 flex-1 truncate text-sm select-all">
          {displayValue}
        </code>
        <div
          className={`ml-2 flex shrink-0 items-center gap-1 transition-opacity ${
            hovered ? "opacity-100" : "opacity-0"
          }`}
        >
          <button
            type="button"
            onClick={() => setRevealed((v) => !v)}
            className="inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground"
            title={revealed ? "隐藏" : "显示"}
          >
            {revealed ? (
              <EyeOff className="h-3.5 w-3.5" />
            ) : (
              <Eye className="h-3.5 w-3.5" />
            )}
          </button>
          <button
            type="button"
            onClick={handleCopy}
            className="inline-flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent hover:text-accent-foreground"
            title="复制"
          >
            {copied ? (
              <Check className="h-3.5 w-3.5 text-green-500" />
            ) : (
              <Copy className="h-3.5 w-3.5" />
            )}
          </button>
        </div>
      </div>
    </div>
  )
}

export function WebServiceSettings() {
  const [status, setStatus] = useState<WebServerInfo | null>(null)
  const [port, setPort] = useState("3080")
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState("")

  const fetchStatus = useCallback(async () => {
    try {
      const info = await getWebServerStatus()
      setStatus(info)
      if (info) {
        setPort(String(info.port))
      }
    } catch {
      // Server status unavailable
    }
  }, [])

  useEffect(() => {
    fetchStatus()
  }, [fetchStatus])

  async function handleStart() {
    setError("")
    setLoading(true)
    try {
      const info = await startWebServer({
        port: parseInt(port, 10) || 3080,
      })
      setStatus(info)
    } catch (e: unknown) {
      const msg =
        e && typeof e === "object" && "message" in e
          ? (e as { message: string }).message
          : "启动失败"
      setError(msg)
    } finally {
      setLoading(false)
    }
  }

  async function handleStop() {
    setLoading(true)
    try {
      await stopWebServer()
      setStatus(null)
    } catch {
      setError("停止失败")
    } finally {
      setLoading(false)
    }
  }

  const isRunning = status !== null

  return (
    <div className="space-y-6">
      <div>
        <h3 className="text-lg font-medium">Web 服务</h3>
        <p className="text-sm text-muted-foreground">
          启用后可通过浏览器远程访问 Codeg
        </p>
      </div>

      <div className="space-y-4">
        {/* Port config */}
        <div className="flex items-center gap-4">
          <label className="w-20 text-sm font-medium">端口</label>
          <input
            type="number"
            value={port}
            onChange={(e) => setPort(e.target.value)}
            disabled={isRunning}
            min={1024}
            max={65535}
            className="flex h-9 w-32 rounded-md border border-input bg-background px-3 py-1 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:opacity-50"
          />
        </div>

        {/* Start/Stop button */}
        <div className="flex items-center gap-4">
          <label className="w-20 text-sm font-medium">状态</label>
          <div className="flex items-center gap-3">
            <span
              className={`inline-block h-2 w-2 rounded-full ${
                isRunning ? "bg-green-500" : "bg-muted-foreground/30"
              }`}
            />
            <span className="text-sm">
              {isRunning ? "运行中" : "已停止"}
            </span>
            <button
              onClick={isRunning ? handleStop : handleStart}
              disabled={loading}
              className="inline-flex h-8 items-center rounded-md border border-input bg-background px-3 text-xs font-medium ring-offset-background transition-colors hover:bg-accent hover:text-accent-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50"
            >
              {loading
                ? "处理中..."
                : isRunning
                  ? "停止"
                  : "启动"}
            </button>
          </div>
        </div>

        {error && (
          <p className="text-sm text-destructive">{error}</p>
        )}

        {/* Connection info */}
        {isRunning && (
          <div className="space-y-3">
            {status.addresses.map((addr) => (
              <AddressCard key={addr} label="访问地址" value={addr} />
            ))}
            <TokenCard label="访问 Token" value={status.token} />
            <p className="text-xs text-muted-foreground">
              Web 客户端首次访问时需输入此 Token
            </p>
          </div>
        )}
      </div>
    </div>
  )
}
