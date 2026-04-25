"use client"

import { useEffect } from "react"
import { useRouter } from "next/navigation"
import { isDesktop } from "@/lib/platform"

export default function Page() {
  const router = useRouter()
  useEffect(() => {
    if (isDesktop()) {
      router.replace("/workspace")
      return
    }
    const token = localStorage.getItem("codeg_token")
    const headers: Record<string, string> = {
      "Content-Type": "application/json",
    }
    if (token) {
      headers.Authorization = `Bearer ${token}`
    }
    fetch("/api/health", {
      method: "POST",
      headers,
      body: "{}",
    })
      .then((res) => {
        if (res.ok) {
          router.replace("/workspace")
        } else {
          localStorage.removeItem("codeg_token")
          router.replace("/login")
        }
      })
      .catch(() => {
        // Server unreachable
        localStorage.removeItem("codeg_token")
        router.replace("/login")
      })
  }, [router])
  return null
}
