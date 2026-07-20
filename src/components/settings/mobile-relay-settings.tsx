"use client"

import { useEffect, useMemo, useState } from "react"
import {
  Check,
  Copy,
  Loader2,
  QrCode,
  RadioTower,
  Server,
  Smartphone,
  Trash2,
} from "lucide-react"
import { QRCodeSVG } from "qrcode.react"

import {
  confirmMobileRelayPairing,
  createMobileRelayPairing,
  getMobileRelayPairingStatus,
  getMobileRelaySettings,
  rejectMobileRelayPairing,
  revokeMobileRelayDevice,
  saveMobileRelaySettings,
  type MobileRelayDevice,
  type MobileRelayPairing,
  type MobileRelayPairingStatus,
  type MobileRelaySettings,
} from "@/lib/api"
import { extractAppCommandError } from "@/lib/app-error"
import { copyTextToClipboard } from "@/lib/utils"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Input } from "@/components/ui/input"
import { Switch } from "@/components/ui/switch"

function errorMessage(error: unknown): string {
  const structured = extractAppCommandError(error)
  if (structured) return structured.detail || structured.message
  return error instanceof Error ? error.message : "操作失败"
}

export function MobileRelaySettingsCard() {
  const [settings, setSettings] = useState<MobileRelaySettings | null>(null)
  const [relayUrl, setRelayUrl] = useState("")
  const [relayToken, setRelayToken] = useState("")
  const [enabled, setEnabled] = useState(false)
  const [deviceName, setDeviceName] = useState("")
  const [pairing, setPairing] = useState<MobileRelayPairing | null>(null)
  const [pairingStatus, setPairingStatus] =
    useState<MobileRelayPairingStatus | null>(null)
  const [revokeTarget, setRevokeTarget] = useState<MobileRelayDevice | null>(
    null
  )
  const [loading, setLoading] = useState(true)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState("")
  const [copied, setCopied] = useState(false)
  const [now, setNow] = useState(() => Math.floor(Date.now() / 1000))

  const refresh = async () => {
    const current = await getMobileRelaySettings()
    setSettings(current)
    setRelayUrl(current.relayUrl)
    setEnabled(current.enabled)
  }

  useEffect(() => {
    let active = true
    void getMobileRelaySettings()
      .then((current) => {
        if (!active) return
        setSettings(current)
        setRelayUrl(current.relayUrl)
        setEnabled(current.enabled)
      })
      .catch((cause) => active && setError(errorMessage(cause)))
      .finally(() => active && setLoading(false))
    return () => {
      active = false
    }
  }, [])

  useEffect(() => {
    if (!pairing) return
    const timer = window.setInterval(
      () => setNow(Math.floor(Date.now() / 1000)),
      1_000
    )
    return () => window.clearInterval(timer)
  }, [pairing])

  useEffect(() => {
    if (!pairing) {
      setPairingStatus(null)
      return
    }
    let active = true
    let polling = false
    const poll = async () => {
      if (polling) return
      polling = true
      try {
        const status = await getMobileRelayPairingStatus(pairing.pairId)
        if (active) {
          setPairingStatus(status)
          if (status.status === "accepted" || status.status === "consumed") {
            setPairing(null)
            setPairingStatus(null)
          }
        }
      } catch (cause) {
        if (active) setError(errorMessage(cause))
      } finally {
        polling = false
      }
    }
    void poll()
    const timer = window.setInterval(() => void poll(), 750)
    return () => {
      active = false
      window.clearInterval(timer)
    }
  }, [pairing])

  const expiresIn = useMemo(
    () => Math.max(0, (pairing?.expiresAt ?? now) - now),
    [now, pairing]
  )
  const relayHost = useMemo(() => {
    try {
      return new URL(relayUrl).host
    } catch {
      return ""
    }
  }, [relayUrl])

  const save = async () => {
    setBusy(true)
    setError("")
    try {
      const current = await saveMobileRelaySettings({
        relayUrl,
        relayToken: relayToken || undefined,
        enabled,
      })
      setSettings(current)
      setRelayToken("")
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(false)
    }
  }

  const createPairing = async () => {
    setBusy(true)
    setError("")
    setCopied(false)
    try {
      const created = await createMobileRelayPairing(deviceName)
      setPairing(created)
      setNow(Math.floor(Date.now() / 1000))
      setDeviceName("")
      await refresh()
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(false)
    }
  }

  const revoke = async () => {
    if (!revokeTarget) return
    setBusy(true)
    setError("")
    try {
      await revokeMobileRelayDevice(revokeTarget.deviceId)
      setRevokeTarget(null)
      await refresh()
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(false)
    }
  }

  const confirmPairing = async () => {
    if (!pairing) return
    setBusy(true)
    setError("")
    try {
      await confirmMobileRelayPairing(pairing.pairId)
      setPairing(null)
      setPairingStatus(null)
      await refresh()
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(false)
    }
  }

  const rejectPairing = async () => {
    if (!pairing) return
    setBusy(true)
    setError("")
    try {
      await rejectMobileRelayPairing(pairing.pairId, pairingStatus?.deviceId)
      setPairing(null)
      setPairingStatus(null)
    } catch (cause) {
      setError(errorMessage(cause))
    } finally {
      setBusy(false)
    }
  }

  const copyPairing = async () => {
    if (!pairing || !(await copyTextToClipboard(pairing.payload))) return
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1_500)
  }

  if (loading) {
    return (
      <section className="flex min-h-32 items-center justify-center rounded-xl border">
        <Loader2 className="h-5 w-5 animate-spin text-muted-foreground" />
      </section>
    )
  }

  const activeDevices =
    settings?.devices.filter((device) => !device.revokedAt) ?? []

  return (
    <>
      <section className="space-y-4 rounded-xl border p-4">
        <div className="flex items-start justify-between gap-4">
          <div className="flex min-w-0 gap-3">
            <div className="flex h-10 w-10 shrink-0 items-center justify-center rounded-xl bg-primary/10 text-primary">
              <RadioTower className="h-5 w-5" />
            </div>
            <div>
              <h4 className="font-medium">手机 Relay 访问</h4>
              <p className="mt-1 text-xs leading-5 text-muted-foreground">
                电脑只建立出站连接，无需公网 IP。命令和 Agent
                事件在手机与电脑之间端到端加密，支持使用任意兼容的自托管 Relay。
              </p>
            </div>
          </div>
          <Switch checked={enabled} onCheckedChange={setEnabled} />
        </div>

        <div className="grid gap-3 md:grid-cols-2">
          <div className="space-y-1.5">
            <label htmlFor="mobile-relay-url" className="text-xs font-medium">
              Relay WebSocket 地址
            </label>
            <Input
              id="mobile-relay-url"
              value={relayUrl}
              onChange={(event) => setRelayUrl(event.target.value)}
              placeholder="wss://relay.example.com/v1/ws"
              autoCapitalize="none"
              autoCorrect="off"
            />
          </div>
          <div className="space-y-1.5">
            <label htmlFor="mobile-relay-token" className="text-xs font-medium">
              桌面 Relay Token
            </label>
            <Input
              id="mobile-relay-token"
              type="password"
              value={relayToken}
              onChange={(event) => setRelayToken(event.target.value)}
              placeholder={
                settings?.relayTokenConfigured
                  ? "已保存在系统钥匙串；留空不修改"
                  : "由 Relay 管理员提供"
              }
              autoComplete="off"
            />
          </div>
        </div>

        <div className="flex gap-3 rounded-xl border border-dashed bg-muted/30 p-3">
          <Server className="mt-0.5 h-4 w-4 shrink-0 text-muted-foreground" />
          <div className="min-w-0 text-xs leading-5 text-muted-foreground">
            <p className="font-medium text-foreground">自托管 Relay</p>
            <p>
              在自己的服务器部署 Codeg Relay，填写它的 WSS 地址和 32 位以上桌面
              Token。公网地址必须使用有效 TLS
              证书；所选域名会写入一次性配对二维码。
            </p>
            {relayHost && (
              <p className="mt-1 truncate font-mono" title={relayHost}>
                当前端点：{relayHost}
              </p>
            )}
          </div>
        </div>

        <div className="flex flex-wrap items-center gap-3">
          <Button onClick={() => void save()} disabled={busy || !relayUrl}>
            {busy && <Loader2 className="h-4 w-4 animate-spin" />}
            保存并应用
          </Button>
          <span className="flex items-center gap-2 text-xs text-muted-foreground">
            <span
              className={`h-2 w-2 rounded-full ${
                settings?.bridgeRunning ? "bg-emerald-500" : "bg-zinc-400"
              }`}
            />
            {settings?.bridgeRunning ? "桥接进程运行中" : "桥接进程未运行"}
          </span>
          {settings?.desktopId && (
            <code className="text-xs text-muted-foreground">
              {settings.desktopId}
            </code>
          )}
        </div>

        {settings?.enabled && settings.bridgeRunning && (
          <div className="space-y-3 border-t pt-4">
            <div>
              <h5 className="text-sm font-medium">添加手机</h5>
              <p className="mt-1 text-xs text-muted-foreground">
                二维码 5 分钟有效，只应在这台电脑前展示。
              </p>
            </div>
            <div className="flex gap-2">
              <Input
                value={deviceName}
                onChange={(event) => setDeviceName(event.target.value)}
                placeholder="设备名称，例如：Crain 的 Android"
                maxLength={80}
              />
              <Button
                variant="outline"
                onClick={() => void createPairing()}
                disabled={busy}
              >
                <QrCode className="h-4 w-4" />
                生成配对码
              </Button>
            </div>
          </div>
        )}

        <div className="space-y-2 border-t pt-4">
          <h5 className="text-sm font-medium">
            已配对设备
            <span className="ml-2 text-xs font-normal text-muted-foreground">
              {activeDevices.length}
            </span>
          </h5>
          {activeDevices.length === 0 ? (
            <p className="rounded-lg bg-muted/40 px-3 py-4 text-center text-xs text-muted-foreground">
              还没有可访问这台电脑的手机
            </p>
          ) : (
            activeDevices.map((device) => (
              <div
                key={device.deviceId}
                className="flex items-center gap-3 rounded-lg border px-3 py-2.5"
              >
                <Smartphone className="h-4 w-4 shrink-0 text-muted-foreground" />
                <div className="min-w-0 flex-1">
                  <div className="truncate text-sm font-medium">
                    {device.name}
                  </div>
                  <div className="truncate font-mono text-[11px] text-muted-foreground">
                    {device.deviceId}
                  </div>
                  <div className="text-[11px] text-muted-foreground">
                    {device.lastSeenAt
                      ? `最后活动：${new Date(
                          device.lastSeenAt * 1000
                        ).toLocaleString()}`
                      : "尚未连接"}
                  </div>
                </div>
                <Button
                  variant="ghost"
                  size="icon"
                  className="text-destructive hover:text-destructive"
                  onClick={() => setRevokeTarget(device)}
                  aria-label={`撤销 ${device.name}`}
                >
                  <Trash2 className="h-4 w-4" />
                </Button>
              </div>
            ))
          )}
        </div>

        {error && (
          <p role="alert" className="text-sm text-destructive">
            {error}
          </p>
        )}
      </section>

      <Dialog
        open={Boolean(pairing)}
        onOpenChange={(open) => !open && void rejectPairing()}
      >
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>扫描并配对手机</DialogTitle>
            <DialogDescription>
              请在 Codeg Mobile 选择 Relay，然后扫描此二维码。
            </DialogDescription>
          </DialogHeader>
          {pairing && (
            <div className="flex flex-col items-center gap-4">
              <div className="rounded-xl bg-white p-3">
                <QRCodeSVG value={pairing.payload} size={232} marginSize={0} />
              </div>
              <p
                className={`text-xs ${
                  expiresIn > 0 ? "text-muted-foreground" : "text-destructive"
                }`}
              >
                {expiresIn > 0
                  ? `${Math.floor(expiresIn / 60)}:${String(
                      expiresIn % 60
                    ).padStart(2, "0")} 后过期`
                  : "配对码已过期，请重新生成"}
              </p>
              <Button
                variant="outline"
                onClick={() => void copyPairing()}
                disabled={
                  expiresIn === 0 || pairingStatus?.status === "requested"
                }
              >
                {copied ? (
                  <Check className="h-4 w-4 text-emerald-500" />
                ) : (
                  <Copy className="h-4 w-4" />
                )}
                {copied ? "已复制" : "复制配对内容"}
              </Button>
              {pairingStatus?.status === "requested" && (
                <div className="w-full space-y-3 rounded-xl border border-primary/30 bg-primary/5 p-4 text-center">
                  <div>
                    <p className="text-sm font-medium">
                      {pairingStatus.deviceName || "Codeg Mobile"} 请求配对
                    </p>
                    <p className="mt-1 text-xs text-muted-foreground">
                      请确认手机显示相同的六位安全码
                    </p>
                  </div>
                  <p className="font-mono text-3xl font-semibold tracking-[0.35em]">
                    {pairingStatus.sas}
                  </p>
                  <div className="grid grid-cols-2 gap-2">
                    <Button
                      variant="outline"
                      onClick={() => void rejectPairing()}
                      disabled={busy}
                    >
                      拒绝
                    </Button>
                    <Button
                      onClick={() => void confirmPairing()}
                      disabled={busy || !pairingStatus.sas}
                    >
                      {busy && <Loader2 className="h-4 w-4 animate-spin" />}
                      确认配对
                    </Button>
                  </div>
                </div>
              )}
              {pairingStatus?.status === "rejected" && (
                <p className="text-sm text-destructive">此次配对已拒绝</p>
              )}
            </div>
          )}
        </DialogContent>
      </Dialog>

      <Dialog
        open={Boolean(revokeTarget)}
        onOpenChange={(open) => !open && setRevokeTarget(null)}
      >
        <DialogContent className="sm:max-w-sm">
          <DialogHeader>
            <DialogTitle>撤销这台手机？</DialogTitle>
            <DialogDescription>
              {revokeTarget?.name} 会立即断开，保存的 Relay
              凭据不能再次访问这台电脑。
            </DialogDescription>
          </DialogHeader>
          <DialogFooter>
            <Button variant="outline" onClick={() => setRevokeTarget(null)}>
              取消
            </Button>
            <Button
              variant="destructive"
              onClick={() => void revoke()}
              disabled={busy}
            >
              {busy && <Loader2 className="h-4 w-4 animate-spin" />}
              确认撤销
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}
