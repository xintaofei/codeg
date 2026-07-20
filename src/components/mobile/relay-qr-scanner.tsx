"use client"

import { useEffect, useRef, useState } from "react"
import { Camera, ImagePlus, Loader2, ScanLine } from "lucide-react"

import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"

export function RelayQrScanner({
  onDetected,
}: {
  onDetected: (payload: string) => void
}) {
  const [open, setOpen] = useState(false)
  const [error, setError] = useState("")
  const [starting, setStarting] = useState(false)
  const videoRef = useRef<HTMLVideoElement>(null)
  const streamRef = useRef<MediaStream | null>(null)
  const frameRef = useRef<number | null>(null)
  const onDetectedRef = useRef(onDetected)

  useEffect(() => {
    onDetectedRef.current = onDetected
  }, [onDetected])

  const stop = () => {
    if (frameRef.current !== null) {
      cancelAnimationFrame(frameRef.current)
      frameRef.current = null
    }
    for (const track of streamRef.current?.getTracks() ?? []) track.stop()
    streamRef.current = null
  }

  useEffect(() => {
    if (!open) {
      stop()
      return
    }
    let active = true
    const start = async () => {
      setStarting(true)
      setError("")
      try {
        const stream = await navigator.mediaDevices.getUserMedia({
          audio: false,
          video: { facingMode: { ideal: "environment" } },
        })
        if (!active) {
          for (const track of stream.getTracks()) track.stop()
          return
        }
        streamRef.current = stream
        const video = videoRef.current
        if (!video) return
        video.srcObject = stream
        await video.play()
        const canvas = document.createElement("canvas")
        const context = canvas.getContext("2d", {
          willReadFrequently: true,
        })
        const decode = (await import("jsqr")).default
        const scan = () => {
          if (!active || !context) return
          if (video.readyState >= HTMLMediaElement.HAVE_CURRENT_DATA) {
            canvas.width = video.videoWidth
            canvas.height = video.videoHeight
            context.drawImage(video, 0, 0, canvas.width, canvas.height)
            const image = context.getImageData(
              0,
              0,
              canvas.width,
              canvas.height
            )
            const result = decode(image.data, image.width, image.height, {
              inversionAttempts: "dontInvert",
            })
            if (result?.data) {
              stop()
              setOpen(false)
              onDetectedRef.current(result.data)
              return
            }
          }
          frameRef.current = requestAnimationFrame(scan)
        }
        frameRef.current = requestAnimationFrame(scan)
      } catch (cause) {
        setError(
          cause instanceof DOMException && cause.name === "NotAllowedError"
            ? "没有相机权限。请在系统设置中允许 Codeg 使用相机，或从相册选择二维码。"
            : "无法打开相机，请从相册选择二维码。"
        )
      } finally {
        if (active) setStarting(false)
      }
    }
    void start()
    return () => {
      active = false
      stop()
    }
  }, [open])

  const decodeFile = async (file: File | undefined) => {
    if (!file) return
    setStarting(true)
    setError("")
    try {
      const bitmap = await createImageBitmap(file)
      const canvas = document.createElement("canvas")
      canvas.width = bitmap.width
      canvas.height = bitmap.height
      const context = canvas.getContext("2d", { willReadFrequently: true })
      if (!context) throw new Error("Canvas is unavailable")
      context.drawImage(bitmap, 0, 0)
      bitmap.close()
      const image = context.getImageData(0, 0, canvas.width, canvas.height)
      const decode = (await import("jsqr")).default
      const result = decode(image.data, image.width, image.height)
      if (!result?.data) throw new Error("QR code not found")
      stop()
      setOpen(false)
      onDetectedRef.current(result.data)
    } catch {
      setError("没有识别到有效的 Codeg Relay 二维码。")
    } finally {
      setStarting(false)
    }
  }

  return (
    <>
      <Button
        type="button"
        variant="outline"
        className="h-12 w-full rounded-xl"
        onClick={() => setOpen(true)}
      >
        <ScanLine className="h-5 w-5" />
        扫描电脑二维码
      </Button>

      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="max-w-sm overflow-hidden">
          <DialogHeader>
            <DialogTitle>扫描 Relay 配对码</DialogTitle>
            <DialogDescription>
              将电脑上的二维码放入取景框，识别后会自动关闭。
            </DialogDescription>
          </DialogHeader>
          <div className="relative aspect-square overflow-hidden rounded-2xl bg-black">
            <video
              ref={videoRef}
              muted
              playsInline
              className="h-full w-full object-cover"
            />
            <div className="pointer-events-none absolute inset-[14%] rounded-2xl border-2 border-white/80 shadow-[0_0_0_999px_rgba(0,0,0,0.35)]" />
            {starting && (
              <div className="absolute inset-0 flex items-center justify-center bg-black/40">
                <Loader2 className="h-7 w-7 animate-spin text-white" />
              </div>
            )}
          </div>
          {error && (
            <p role="alert" className="text-sm text-destructive">
              {error}
            </p>
          )}
          <label className="inline-flex h-12 cursor-pointer items-center justify-center gap-2 rounded-xl border border-input bg-background px-4 text-sm font-medium hover:bg-accent">
            <ImagePlus className="h-5 w-5" />
            从相册选择二维码
            <input
              type="file"
              accept="image/*"
              className="sr-only"
              onChange={(event) => {
                void decodeFile(event.target.files?.[0])
                event.currentTarget.value = ""
              }}
            />
          </label>
          <p className="flex items-center justify-center gap-1 text-center text-xs text-muted-foreground">
            <Camera className="h-3.5 w-3.5" />
            相机画面只在本机处理，不会上传
          </p>
        </DialogContent>
      </Dialog>
    </>
  )
}
