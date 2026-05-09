// Transport-aware bindings for the pet command surface.
// All functions go through `getTransport().call(...)` so the same code runs
// in Tauri (`invoke`) and standalone-server (`fetch`) modes.

import { getTransport } from "@/lib/transport"
import type {
  ImportablePet,
  ImportCodexPetsRequest,
  ImportCodexPetsResult,
  NewPetInput,
  PetCodexImportAvailability,
  PetDetail,
  PetMetaPatch,
  PetSpriteAsset,
  PetSummary,
  PetWindowConfig,
  PetWindowStatePatch,
} from "./types"

export async function listPets(): Promise<PetSummary[]> {
  return getTransport().call("pet_list")
}

export async function getPet(id: string): Promise<PetDetail> {
  return getTransport().call("pet_get", { id })
}

export async function readPetSpritesheet(id: string): Promise<PetSpriteAsset> {
  return getTransport().call("pet_read_spritesheet", { id })
}

export async function addPet(input: NewPetInput): Promise<PetSummary> {
  return getTransport().call("pet_add", { ...input })
}

export async function updatePetMeta(
  id: string,
  patch: PetMetaPatch
): Promise<PetSummary> {
  return getTransport().call("pet_update_meta", { id, patch })
}

export async function replacePetSprite(
  id: string,
  spritesheetBase64: string
): Promise<void> {
  return getTransport().call("pet_replace_sprite", { id, spritesheetBase64 })
}

export async function deletePet(id: string): Promise<void> {
  return getTransport().call("pet_delete", { id })
}

export async function listImportableCodexPets(): Promise<ImportablePet[]> {
  return getTransport().call("pet_list_importable_codex")
}

export async function importCodexPets(
  request: ImportCodexPetsRequest
): Promise<ImportCodexPetsResult> {
  return getTransport().call("pet_import_codex", { ...request })
}

export async function isCodexImportAvailable(): Promise<PetCodexImportAvailability> {
  return getTransport().call("pet_codex_import_available")
}

export async function getPetSettings(): Promise<PetWindowConfig> {
  return getTransport().call("pet_get_settings")
}

export async function setActivePet(
  petId: string | null
): Promise<PetWindowConfig> {
  return getTransport().call("pet_set_active", { petId })
}

export async function savePetWindowState(
  patch: PetWindowStatePatch
): Promise<PetWindowConfig> {
  return getTransport().call("pet_save_window_state", { ...patch })
}

// Tauri-only — these are noops in web mode (the standalone server cannot
// open native windows on the user's machine). Callers should branch on
// `isDesktop()` before invoking them.
export async function openPetWindow(): Promise<void> {
  return getTransport().call("open_pet_window")
}

export async function closePetWindow(): Promise<void> {
  return getTransport().call("close_pet_window")
}

export async function recordPetWindowPosition(
  x: number,
  y: number
): Promise<void> {
  return getTransport().call("pet_window_record_position", { x, y })
}

// Slug a free-form display name into a pet id. Mirrors the backend's
// validate_pet_id rules: lowercase ASCII, digits, '-' and '_'. Used as a
// suggestion when adding a pet — the user can override.
export function slugifyPetId(input: string): string {
  return input
    .normalize("NFKD")
    .replace(/[̀-ͯ]/g, "")
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 64)
}
