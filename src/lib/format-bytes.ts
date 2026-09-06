/**
 * Human-readable byte count for info-only readouts (cache stats, file sizes).
 * Shared because a second settings surface needed the same three-tier
 * formatting; a drift here would show "1024.0 KB" on one page and "1.0 MB"
 * on another.
 */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}
