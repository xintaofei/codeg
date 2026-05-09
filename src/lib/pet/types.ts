// Mirrors of the Rust pet types used over `getTransport().call(...)`. Kept
// in their own file so the animation runtime can import them without pulling
// in the API layer.

export interface PetSummary {
  id: string
  displayName: string
  description: string | null
  spritesheetPath: string
}

export interface PetDetail {
  id: string
  displayName: string
  description: string | null
  spritesheetPath: string
}

export interface PetSpriteAsset {
  mime: string
  dataBase64: string
}

export interface NewPetInput {
  id: string
  displayName: string
  description?: string | null
  spritesheetBase64: string
}

export interface PetMetaPatch {
  displayName?: string
  // Two-level optional matches the Rust `Option<Option<String>>`. Sending
  // `null` clears the description; omitting the key leaves it untouched.
  description?: string | null
}

export interface PetWindowConfig {
  enabled: boolean
  activePetId: string | null
  x: number | null
  y: number | null
  scale: number
  alwaysOnTop: boolean
}

export interface PetWindowStatePatch {
  x?: number
  y?: number
  scale?: number
  alwaysOnTop?: boolean
  enabled?: boolean
}

export interface ImportablePet {
  id: string
  displayName: string
  description: string | null
  sourcePath: string
  alreadyImported: boolean
}

export interface ImportCodexPetsRequest {
  ids?: string[]
  overwriteWithSuffix?: boolean
}

export interface ImportCodexPetsResult {
  importedIds: string[]
  skipped: ImportSkipped[]
}

export interface ImportSkipped {
  sourceId: string
  reason: string
}

export interface PetCodexImportAvailability {
  available: boolean
}
