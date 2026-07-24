export interface PartitionedAttachments<T> {
  images: T[]
  resources: T[]
}

const MIME_BY_EXT: Record<string, string> = {
  txt: "text/plain",
  md: "text/markdown",
  json: "application/json",
  yaml: "application/yaml",
  yml: "application/yaml",
  csv: "text/csv",
  html: "text/html",
  css: "text/css",
  js: "text/javascript",
  mjs: "text/javascript",
  cjs: "text/javascript",
  ts: "text/typescript",
  tsx: "text/tsx",
  jsx: "text/jsx",
  py: "text/x-python",
  rs: "text/rust",
  go: "text/x-go",
  java: "text/x-java-source",
  xml: "application/xml",
  toml: "application/toml",
  pdf: "application/pdf",
  png: "image/png",
  jpg: "image/jpeg",
  jpeg: "image/jpeg",
  gif: "image/gif",
  webp: "image/webp",
  svg: "image/svg+xml",
}

export function mimeTypeFromPath(path: string): string | null {
  const ext = path.split(".").pop()?.toLowerCase() ?? ""
  return MIME_BY_EXT[ext] ?? null
}

function partition<T>(
  items: T[],
  canAttachImages: boolean,
  getMimeType: (item: T) => string | null
): PartitionedAttachments<T> {
  const result: PartitionedAttachments<T> = { images: [], resources: [] }
  for (const item of items) {
    const isImage = getMimeType(item)?.startsWith("image/") ?? false
    if (canAttachImages && isImage) {
      result.images.push(item)
    } else {
      result.resources.push(item)
    }
  }
  return result
}

export function partitionAttachmentFiles(
  files: File[],
  canAttachImages: boolean
): PartitionedAttachments<File> {
  return partition(
    files,
    canAttachImages,
    (file) => file.type || mimeTypeFromPath(file.name)
  )
}

export function partitionAttachmentPaths(
  paths: string[],
  canAttachImages: boolean
): PartitionedAttachments<string> {
  return partition(paths, canAttachImages, mimeTypeFromPath)
}
