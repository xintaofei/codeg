import { useCallback, useEffect, useRef, useState } from "react"

export type VoiceInputStatus = "idle" | "listening" | "transcribing" | "error"
export type VoiceInputMode = "web-speech" | "whisper"

export interface UseVoiceInputOptions {
  /** Called when a speech segment is transcribed. */
  onTranscript?: (text: string, isFinal: boolean) => void
  /** Called with the real-time interim recognition text. */
  onInterimTranscript?: (text: string) => void
  /** Called when an error occurs during speech recognition. */
  onError?: (error: string) => void
  /** BCP 47 language tag (e.g. 'zh-CN', 'en-US'). Defaults to navigator.language. */
  lang?: string
  /** Optional Whisper/ASR transcription API endpoint (e.g. '/v1/audio/transcriptions'). */
  asrEndpoint?: string
  /** Optional API token for the ASR endpoint. */
  asrApiKey?: string
}

export interface UseVoiceInputReturn {
  status: VoiceInputStatus
  mode: VoiceInputMode
  isSupported: boolean
  volume: number
  interimText: string
  errorMessage: string | null
  startListening: () => Promise<void>
  stopListening: () => void
  cancel: () => void
}

export function useVoiceInput(options: UseVoiceInputOptions = {}): UseVoiceInputReturn {
  const { onTranscript, onInterimTranscript, onError, lang, asrEndpoint, asrApiKey } = options

  const [status, setStatus] = useState<VoiceInputStatus>("idle")
  const [mode, setMode] = useState<VoiceInputMode>("web-speech")
  const [volume, setVolume] = useState<number>(0)
  const [interimText, setInterimText] = useState<string>("")
  const [errorMessage, setErrorMessage] = useState<string | null>(null)

  const recognitionRef = useRef<any>(null)
  const mediaStreamRef = useRef<MediaStream | null>(null)
  const mediaRecorderRef = useRef<MediaRecorder | null>(null)
  const audioChunksRef = useRef<Blob[]>([])
  const audioContextRef = useRef<AudioContext | null>(null)
  const animationFrameRef = useRef<number | null>(null)
  const isMountedRef = useRef(true)

  useEffect(() => {
    isMountedRef.current = true
    return () => {
      isMountedRef.current = false
    }
  }, [])

  // Check if Web Speech API or MediaDevices is supported
  const isSupported = typeof window !== "undefined" && Boolean(
    ("SpeechRecognition" in window || "webkitSpeechRecognition" in window) ||
    (navigator.mediaDevices && typeof navigator.mediaDevices.getUserMedia === "function")
  )

  const cleanupAudio = useCallback(() => {
    if (animationFrameRef.current !== null) {
      cancelAnimationFrame(animationFrameRef.current)
      animationFrameRef.current = null
    }
    if (audioContextRef.current && audioContextRef.current.state !== "closed") {
      void audioContextRef.current.close().catch(() => {})
      audioContextRef.current = null
    }
    if (mediaStreamRef.current) {
      mediaStreamRef.current.getTracks().forEach((track) => track.stop())
      mediaStreamRef.current = null
    }
    setVolume(0)
  }, [])

  const cancel = useCallback(() => {
    if (recognitionRef.current) {
      try {
        recognitionRef.current.abort()
      } catch {}
      recognitionRef.current = null
    }
    if (mediaRecorderRef.current && mediaRecorderRef.current.state !== "inactive") {
      try {
        mediaRecorderRef.current.stop()
      } catch {}
      mediaRecorderRef.current = null
    }
    cleanupAudio()
    setInterimText("")
    setStatus("idle")
  }, [cleanupAudio])

  const startVolumeMeter = useCallback((stream: MediaStream) => {
    try {
      const AudioContextClass = window.AudioContext || (window as any).webkitAudioContext
      if (!AudioContextClass) return
      const audioCtx = new AudioContextClass()
      audioContextRef.current = audioCtx
      const source = audioCtx.createMediaStreamSource(stream)
      const analyser = audioCtx.createAnalyser()
      analyser.fftSize = 256
      source.connect(analyser)

      const dataArray = new Uint8Array(analyser.frequencyBinCount)
      const checkVolume = () => {
        if (!isMountedRef.current) return
        analyser.getByteFrequencyData(dataArray)
        let sum = 0
        for (let i = 0; i < dataArray.length; i++) {
          sum += dataArray[i]
        }
        const avg = sum / dataArray.length
        const norm = Math.min(1, Math.max(0, avg / 80))
        setVolume(norm)
        animationFrameRef.current = requestAnimationFrame(checkVolume)
      }
      checkVolume()
    } catch (e) {
      console.warn("[useVoiceInput] Volume meter failed:", e)
    }
  }, [])

  const startListening = useCallback(async () => {
    setErrorMessage(null)
    setInterimText("")

    const SpeechRecognitionClass = (typeof window !== "undefined" && ((window as any).SpeechRecognition || (window as any).webkitSpeechRecognition)) || null

    if (SpeechRecognitionClass && !asrEndpoint) {
      setMode("web-speech")
      try {
        if (navigator.mediaDevices?.getUserMedia) {
          try {
            const stream = await navigator.mediaDevices.getUserMedia({ audio: true })
            mediaStreamRef.current = stream
            startVolumeMeter(stream)
          } catch {}
        }

        const recognition = new SpeechRecognitionClass()
        recognitionRef.current = recognition
        recognition.continuous = true
        recognition.interimResults = true
        recognition.lang = lang || (typeof navigator !== "undefined" ? navigator.language : "zh-CN") || "zh-CN"

        recognition.onresult = (event: any) => {
          let finalChunk = ""
          let currentInterim = ""

          for (let i = event.resultIndex; i < event.results.length; i++) {
            const transcript = event.results[i][0].transcript
            if (event.results[i].isFinal) {
              finalChunk += transcript
            } else {
              currentInterim += transcript
            }
          }

          if (finalChunk) {
            onTranscript?.(finalChunk, true)
          }
          setInterimText(currentInterim)
          onInterimTranscript?.(currentInterim)
        }

        recognition.onerror = (event: any) => {
          console.warn("[useVoiceInput] recognition error:", event.error)
          if (event.error === "no-speech") return
          const err = event.error === "not-allowed" ? "micPermissionDenied" : event.error
          setErrorMessage(err)
          onError?.(err)
          setStatus("error")
          cleanupAudio()
        }

        recognition.onend = () => {
          cleanupAudio()
          setStatus("idle")
          setInterimText("")
        }

        recognition.start()
        setStatus("listening")
      } catch (err: any) {
        console.error("[useVoiceInput] start failed:", err)
        setErrorMessage(err?.message || "Failed to start speech recognition")
        setStatus("error")
        cleanupAudio()
      }
      return
    }

    if (navigator.mediaDevices?.getUserMedia) {
      setMode("whisper")
      try {
        const stream = await navigator.mediaDevices.getUserMedia({ audio: true })
        mediaStreamRef.current = stream
        startVolumeMeter(stream)

        const recorder = new MediaRecorder(stream)
        mediaRecorderRef.current = recorder
        audioChunksRef.current = []

        recorder.ondataavailable = (e) => {
          if (e.data && e.data.size > 0) {
            audioChunksRef.current.push(e.data)
          }
        }

        recorder.start(250)
        setStatus("listening")
      } catch (err: any) {
        const msg = err.name === "NotAllowedError" ? "micPermissionDenied" : err.message
        setErrorMessage(msg)
        onError?.(msg)
        setStatus("error")
        cleanupAudio()
      }
      return
    }

    setErrorMessage("speechNotSupported")
    onError?.("speechNotSupported")
    setStatus("error")
  }, [asrEndpoint, lang, onTranscript, onInterimTranscript, onError, startVolumeMeter, cleanupAudio])

  const stopListening = useCallback(() => {
    if (recognitionRef.current) {
      try {
        recognitionRef.current.stop()
      } catch {}
      if (interimText.trim()) {
        onTranscript?.(interimText.trim(), true)
        setInterimText("")
      }
      cleanupAudio()
      setStatus("idle")
      return
    }

    if (mediaRecorderRef.current && mediaRecorderRef.current.state !== "inactive") {
      setStatus("transcribing")
      mediaRecorderRef.current.onstop = async () => {
        cleanupAudio()
        const audioBlob = new Blob(audioChunksRef.current, { type: "audio/webm" })
        audioChunksRef.current = []

        if (asrEndpoint) {
          try {
            const formData = new FormData()
            formData.append("file", audioBlob, "audio.webm")
            formData.append("model", "whisper-1")
            formData.append("language", lang?.startsWith("zh") ? "zh" : "en")

            const res = await fetch(asrEndpoint, {
              method: "POST",
              headers: asrApiKey ? { Authorization: "Bearer " + asrApiKey } : {},
              body: formData,
            })
            if (!res.ok) throw new Error("ASR HTTP " + res.status)
            const data = await res.json()
            const text = data.text || ""
            if (text) {
              onTranscript?.(text, true)
            }
          } catch (e: any) {
            setErrorMessage(e.message)
            onError?.(e.message)
          }
        }
        setStatus("idle")
      }
      try {
        mediaRecorderRef.current.stop()
      } catch {
        cleanupAudio()
        setStatus("idle")
      }
    }
  }, [interimText, onTranscript, asrEndpoint, asrApiKey, lang, onError, cleanupAudio])

  return {
    status,
    mode,
    isSupported,
    volume,
    interimText,
    errorMessage,
    startListening,
    stopListening,
    cancel,
  }
}
