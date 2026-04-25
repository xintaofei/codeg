"use client"

import { useEffect, useState } from "react"
import { useRouter } from "next/navigation"
import { isDesktop } from "@/lib/platform"

export default function LoginPage() {
  const router = useRouter()
  const [token, setToken] = useState("")
  const [error, setError] = useState("")
  const [loading, setLoading] = useState(false)

  useEffect(() => {
    document.title = "Login - codeg"
    fetch("/api/health", {
      method: "POST",
      headers: {
        "Content-Type": "application/json",
      },
      body: "{}",
    })
      .then((res) => {
        if (res.ok) {
          localStorage.removeItem("codeg_token")
          router.replace("/workspace")
        }
      })
      .catch(() => {
        // keep login form for protected mode or unreachable server
      })
  }, [router])

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
      // Validate token by calling a lightweight API endpoint
      const res = await fetch("/api/health", {
        method: "POST",
        headers: token
          ? {
              "Content-Type": "application/json",
              Authorization: `Bearer ${token}`,
            }
          : {
              "Content-Type": "application/json",
            },
        body: "{}",
      })

      if (res.ok) {
        if (token) {
          localStorage.setItem("codeg_token", token)
        } else {
          localStorage.removeItem("codeg_token")
        }
        router.replace("/workspace")
      } else if (res.status === 401) {
        setError("Token 无效，请检查后重试")
      } else {
        setError(`连接失败 (HTTP ${res.status})`)
      }
    } catch {
      setError("无法连接到服务器")
    } finally {
      setLoading(false)
    }
  }

  return (
    <div className="flex min-h-screen items-center justify-center bg-background">
      <div className="w-full max-w-sm space-y-6 px-4">
        <div className="space-y-2 text-center">
          <h1 className="text-2xl font-bold tracking-tight">Codeg</h1>
          <p className="text-sm text-muted-foreground">
            输入访问 Token 以连接到桌面端
          </p>
        </div>

        <form onSubmit={handleSubmit} className="space-y-4">
          <div className="space-y-2">
            <input
              type="password"
              value={token}
              onChange={(e) => setToken(e.target.value)}
              placeholder="Access Token"
              autoFocus
              className="flex h-10 w-full rounded-md border border-input bg-background px-3 py-2 text-base md:text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
            />
          </div>

          {error && <p className="text-sm text-destructive">{error}</p>}

          <button
            type="submit"
            disabled={loading}
            className="inline-flex h-10 w-full items-center justify-center rounded-md bg-primary px-4 py-2 text-sm font-medium text-primary-foreground ring-offset-background transition-colors hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring disabled:pointer-events-none disabled:opacity-50"
          >
            {loading ? "连接中..." : "连接"}
          </button>
        </form>

        <p className="text-center text-xs text-muted-foreground">
          Token 可在桌面端 设置 → Web 服务 中获取
        </p>
      </div>
    </div>
  )
}
