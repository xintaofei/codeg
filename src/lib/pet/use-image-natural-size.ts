import { useEffect, useState } from "react"

export interface ImageNaturalSize {
  width: number
  height: number
}

interface MeasuredState {
  url: string | null
  size: ImageNaturalSize | null
}

/**
 * Measure an image's intrinsic pixel size by loading it off-DOM. Returns
 * `null` until the image for the current `url` has loaded (or if it fails).
 *
 * Sprite sheets grew a format version — v1 pets are a 9-row 1536×1872 sheet,
 * v2 pets an 11-row 1536×2288 one — so the row/frame count can no longer be
 * assumed. Callers derive geometry from the measured size (see
 * `spriteRowsFromHeight` / `filmstripFrameCount`) and fall back to the legacy
 * layout while `null`, which keeps the 9 base states correct either way.
 *
 * The result is keyed by URL, so when `url` changes we report `null` until the
 * new image resolves rather than briefly painting the previous sheet's
 * dimensions onto it.
 */
export function useImageNaturalSize(
  url: string | null | undefined
): ImageNaturalSize | null {
  const [state, setState] = useState<MeasuredState>({ url: null, size: null })

  useEffect(() => {
    if (!url) return

    let cancelled = false
    const img = new Image()
    const apply = () => {
      if (cancelled) return
      const { naturalWidth: width, naturalHeight: height } = img
      setState({
        url,
        size: width > 0 && height > 0 ? { width, height } : null,
      })
    }
    img.onload = apply
    img.onerror = () => {
      if (!cancelled) setState({ url, size: null })
    }
    img.src = url
    // A cached image may already be decoded; resolve on the next microtask so
    // we never call setState synchronously inside the effect body.
    if (img.complete) queueMicrotask(apply)

    return () => {
      cancelled = true
      img.onload = null
      img.onerror = null
    }
  }, [url])

  return state.url === url ? state.size : null
}
