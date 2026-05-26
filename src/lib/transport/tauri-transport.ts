import type { CallOptions, Transport, UnsubscribeFn } from "./types"

type TauriEventListenersWindow = {
  __TAURI_EVENT_PLUGIN_INTERNALS__?: {
    unregisterListener?: (event: string, eventId: number) => void
  }
}

export class TauriTransport implements Transport {
  async call<T>(
    command: string,
    args?: Record<string, unknown>,
    options?: CallOptions
  ): Promise<T> {
    // Tauri invoke() has no client-side timeout — the IPC channel runs
    // for as long as the command needs. `options.timeoutMs` is part
    // of the Transport contract for web-mode parity, ignored here.
    void options
    const { invoke } = await import("@tauri-apps/api/core")
    return invoke(command, args)
  }

  // Bypasses `@tauri-apps/api/event#listen` to sidestep an intermittent race:
  // Tauri's Rust `listen_js` schedules a fire-and-forget eval that populates
  // `window.__TAURI_EVENT_LISTENERS__[event][eventId]` and, separately, returns
  // `eventId` via the invoke response. On WKWebView the two can arrive out of
  // order. When cleanup fires before the eval lands, the built-in `_unlisten`
  // throws synchronously on `listeners[eventId].handlerId` — and because that
  // throw happens BEFORE `await invoke('plugin:event|unlisten')`, the backend
  // listener is never removed (handler + payload buffering leak).
  //
  // We own the eventId here so we can always issue the backend unlisten, even
  // when the client-side registry entry hasn't appeared yet.
  async subscribe<T>(
    event: string,
    handler: (payload: T) => void
  ): Promise<UnsubscribeFn> {
    const { invoke, transformCallback } = await import("@tauri-apps/api/core")
    const handlerId = transformCallback((e: { payload: T }) => {
      handler(e.payload)
    })
    const eventId = await invoke<number>("plugin:event|listen", {
      event,
      target: { kind: "Any" },
      handler: handlerId,
    })

    let unlistened = false
    return () => {
      if (unlistened) return
      unlistened = true
      try {
        const internals = (window as unknown as TauriEventListenersWindow)
          .__TAURI_EVENT_PLUGIN_INTERNALS__
        internals?.unregisterListener?.(event, eventId)
      } catch {
        // Registration eval has not landed yet; server-side unlisten below
        // still clears the listener so events stop flowing.
      }
      // Fire-and-forget: callers expect a sync unsubscribe, and a failure here
      // only means the backend already forgot this listener.
      invoke("plugin:event|unlisten", { event, eventId }).catch(() => {})
    }
  }

  isDesktop(): boolean {
    return true
  }
}
