import { invoke } from "@tauri-apps/api/core"

export async function storeSecret(key: string, value: string): Promise<void> {
  await invoke("plugin:secure-vault|store_secret", {
    payload: { key, value },
  })
}

export async function loadSecret(key: string): Promise<string | null> {
  return invoke<{ value?: string }>("plugin:secure-vault|load_secret", {
    payload: { key },
  }).then((result) => result.value ?? null)
}

export async function deleteSecret(key: string): Promise<void> {
  await invoke("plugin:secure-vault|delete_secret", {
    payload: { key },
  })
}
