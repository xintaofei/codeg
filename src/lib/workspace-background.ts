// Transport-aware bindings + shared model for the workspace background image.
//
// The image bytes live on disk under `~/.codeg/backgrounds/` (backend
// `crate::backgrounds`), read/written through `getTransport().call(...)` so the
// same code runs in Tauri (`invoke`) and standalone-server (`fetch`) modes —
// mirroring `src/lib/pet/api.ts`. The lightweight display config (enabled,
// mask, blur, fill, panel opacity) lives in localStorage via the appearance
// provider; only the image itself round-trips through here.

import { getTransport } from "@/lib/transport"

// ─── Types ───

export type WorkspaceBgFillMode = "cover" | "contain" | "center" | "tile"

/** camelCase mirror of the Rust `BackgroundAsset` returned by `background_read`. */
export type BackgroundAsset = { mime: string; dataBase64: string }

// ─── Presets / defaults / ranges ───

export const WORKSPACE_BG_FILL_MODES = [
  "cover",
  "contain",
  "center",
  "tile",
] as const satisfies readonly WorkspaceBgFillMode[]

/** CSS `background-size` / `background-repeat` for each fill mode. */
export const FILL_MODE_STYLE: Record<
  WorkspaceBgFillMode,
  { size: string; repeat: string }
> = {
  cover: { size: "cover", repeat: "no-repeat" },
  contain: { size: "contain", repeat: "no-repeat" },
  center: { size: "auto", repeat: "no-repeat" },
  tile: { size: "auto", repeat: "repeat" },
}

export const DEFAULT_WORKSPACE_BG_ENABLED = false
export const DEFAULT_WORKSPACE_BG_MASK_OPACITY = 0.82
export const DEFAULT_WORKSPACE_BG_IMAGE_BLUR = 0
export const DEFAULT_WORKSPACE_BG_PANEL_OPACITY = 0.3
export const DEFAULT_WORKSPACE_BG_FILL_MODE: WorkspaceBgFillMode = "cover"

export const WORKSPACE_BG_MASK_OPACITY_RANGE = { min: 0, max: 0.99, step: 0.01 }
export const WORKSPACE_BG_IMAGE_BLUR_RANGE = { min: 0, max: 24, step: 1 }
export const WORKSPACE_BG_PANEL_OPACITY_RANGE = { min: 0, max: 1, step: 0.01 }

/** Client-side upload ceiling; matches the backend `MAX_BG_BYTES` (16 MiB). */
export const MAX_WORKSPACE_BG_BYTES = 16 * 1024 * 1024

// ─── Validation / clamp ───

function clamp(v: number, min: number, max: number): number {
  if (Number.isNaN(v)) return min
  return Math.min(max, Math.max(min, v))
}

export function clampMaskOpacity(v: number): number {
  return clamp(
    v,
    WORKSPACE_BG_MASK_OPACITY_RANGE.min,
    WORKSPACE_BG_MASK_OPACITY_RANGE.max
  )
}

export function clampImageBlur(v: number): number {
  return clamp(
    v,
    WORKSPACE_BG_IMAGE_BLUR_RANGE.min,
    WORKSPACE_BG_IMAGE_BLUR_RANGE.max
  )
}

export function clampPanelOpacity(v: number): number {
  return clamp(
    v,
    WORKSPACE_BG_PANEL_OPACITY_RANGE.min,
    WORKSPACE_BG_PANEL_OPACITY_RANGE.max
  )
}

export function isValidFillMode(v: unknown): v is WorkspaceBgFillMode {
  return (
    typeof v === "string" &&
    (WORKSPACE_BG_FILL_MODES as readonly string[]).includes(v)
  )
}

// ─── base64 / blob-URL helpers (equivalents of the pet sprite helpers) ───

export function arrayBufferToBase64(bytes: Uint8Array): string {
  let binary = ""
  const chunk = 0x8000
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode.apply(
      null,
      Array.from(bytes.subarray(i, i + chunk))
    )
  }
  return btoa(binary)
}

export function createBackgroundObjectUrl(asset: BackgroundAsset): string {
  const binary = atob(asset.dataBase64)
  const bytes = new Uint8Array(binary.length)
  for (let i = 0; i < binary.length; i += 1) {
    bytes[i] = binary.charCodeAt(i)
  }
  return URL.createObjectURL(new Blob([bytes], { type: asset.mime }))
}

export function revokeBackgroundObjectUrl(
  url: string | null | undefined
): void {
  if (url?.startsWith("blob:")) {
    URL.revokeObjectURL(url)
  }
}

// ─── Transport bindings (dual-mode, mirrors src/lib/pet/api.ts) ───

/** Returns the stored background asset, or `null` when none is set. */
export async function readWorkspaceBackground(): Promise<BackgroundAsset | null> {
  return getTransport().call("background_read")
}

export async function setWorkspaceBackground(
  imageBase64: string
): Promise<void> {
  return getTransport().call("background_set", { imageBase64 })
}

export async function clearWorkspaceBackground(): Promise<void> {
  return getTransport().call("background_clear")
}
