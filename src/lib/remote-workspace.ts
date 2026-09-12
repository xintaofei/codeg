import { getShellTransport } from "@/lib/transport"
import type {
  DirectoryEntry,
  RemoteWorkspaceConnection,
  RemoteWorkspaceConnectionInput,
} from "@/lib/types"

export async function listRemoteWorkspaceConnections(): Promise<
  RemoteWorkspaceConnection[]
> {
  return getShellTransport().call("list_remote_workspace_connections")
}

export async function getRemoteWorkspaceConnection(
  id: number
): Promise<RemoteWorkspaceConnection> {
  return getShellTransport().call("get_remote_workspace_connection", { id })
}

export async function testRemoteWorkspaceConnection(
  input: RemoteWorkspaceConnectionInput
): Promise<void> {
  return getShellTransport().call("test_remote_workspace_connection", { input })
}

export async function createRemoteWorkspaceConnection(
  input: RemoteWorkspaceConnectionInput
): Promise<RemoteWorkspaceConnection> {
  return getShellTransport().call("create_remote_workspace_connection", {
    input,
  })
}

export async function updateRemoteWorkspaceConnection(
  id: number,
  input: RemoteWorkspaceConnectionInput
): Promise<RemoteWorkspaceConnection> {
  return getShellTransport().call("update_remote_workspace_connection", {
    id,
    input,
  })
}

export async function deleteRemoteWorkspaceConnection(
  id: number
): Promise<void> {
  return getShellTransport().call("delete_remote_workspace_connection", { id })
}

export async function reorderRemoteWorkspaceConnections(
  ids: number[]
): Promise<void> {
  return getShellTransport().call("reorder_remote_workspace_connections", {
    ids,
  })
}

export async function openRemoteWorkspace(id: number): Promise<void> {
  return getShellTransport().call("open_remote_workspace", { id })
}

/**
 * Tauri event the local window uses to hand an already-open remote workspace
 * window a folder to open. MUST match `REMOTE_OPEN_FOLDER_EVENT` in
 * `src-tauri/src/commands/remote_workspace.rs`.
 */
export const REMOTE_OPEN_FOLDER_EVENT = "remote-open-folder"

/**
 * Run one command on a specific remote workspace, bypassing this window's own
 * transport.
 *
 * A window's transport is bound to exactly one backend — the local machine,
 * or the remote server this window was opened against. Browsing *another*
 * workspace (to pick a folder on it before deciding to switch) has to talk to
 * that server directly, so these calls go through the local `remote_http_call`
 * proxy instead of `getTransport()`.
 *
 * Desktop-only: the proxy is a Tauri command, so a web client (already talking
 * to one server of its own) has nothing to proxy through.
 */
function remoteCall<T>(
  connectionId: number,
  command: string,
  args?: Record<string, unknown>
): Promise<T> {
  return getShellTransport().call<T>("remote_http_call", {
    connectionId,
    command,
    args: args ?? {},
  })
}

/** List a directory on a remote workspace host. */
export async function listRemoteDirectoryEntries(
  connectionId: number,
  path: string
): Promise<DirectoryEntry[]> {
  return remoteCall<DirectoryEntry[]>(connectionId, "list_directory_entries", {
    path,
  })
}

/** Home directory of the user running a remote codeg-server. */
export async function getRemoteHomeDirectory(
  connectionId: number
): Promise<string> {
  return remoteCall<string>(connectionId, "get_home_directory")
}

/**
 * Open `path` in its own workspace window: focuses the window bound to this
 * connection (spawning it if needed) and hands it the path to open. The folder
 * is opened *there*, by that window, so it lands in the backend that owns it.
 */
export async function openRemoteWorkspaceFolder(
  id: number,
  path: string
): Promise<void> {
  return getShellTransport().call("open_remote_workspace_folder", { id, path })
}
