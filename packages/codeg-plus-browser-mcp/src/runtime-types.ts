export type BrowserRuntimeState =
  | "stopped"
  | "starting"
  | "ready"
  | "recovering"
  | "error"

export interface BrowserRuntimeStatus {
  state: BrowserRuntimeState
  browserPid: number | null
  browserName: string | null
  browserVersion: string | null
  sessionCount: number
  recoveryAttempt: number
  lastErrorCode: string | null
}

export interface BrowserDoctorResult {
  ok: boolean
  checks: Array<{
    name: "browser" | "profile" | "downloads" | "cdp" | "process"
    ok: boolean
    detail: string
  }>
}

export interface BrowserTab {
  id: string
  url: string
  title: string
}

export interface BrowserBackendSessionSnapshot {
  tabs: BrowserTab[]
  activeTargetId: string | null
}

export interface BrowserSurfacePageState {
  tab: BrowserTab
  loading: boolean
  canGoBack: boolean
  canGoForward: boolean
}

export interface BrowserSurfaceSnapshot {
  sessionId: string
  tabs: BrowserTab[]
  activeTargetId: string | null
  active: BrowserSurfacePageState | null
}

export interface BrowserSurfaceFrame {
  targetId: string
  data: string
  mimeType: "image/jpeg"
  deviceWidth: number
  deviceHeight: number
  pageScaleFactor: number
}

export type BrowserSurfaceEvent =
  | { type: "snapshot"; snapshot: BrowserSurfaceSnapshot }
  | { type: "frame"; frame: BrowserSurfaceFrame }
  | {
      type: "error"
      code: string
      message: string
      retryable: boolean
    }

export type BrowserSurfaceInput =
  | {
      kind: "mouse"
      event: "pressed" | "released" | "moved" | "wheel"
      x: number
      y: number
      button?: "none" | "left" | "middle" | "right"
      deltaX?: number
      deltaY?: number
      modifiers?: number
    }
  | {
      kind: "key"
      event: "down" | "up"
      key: string
      code?: string
      text?: string
      modifiers?: number
    }
  | { kind: "text"; text: string }

export type BrowserSurfaceAction =
  | { action: "open"; url?: string }
  | { action: "focus"; targetId: string }
  | { action: "close"; targetId: string }
  | { action: "resize"; width: number; height: number }
  | { action: "navigate"; url: string }
  | { action: "back" }
  | { action: "forward" }
  | { action: "reload" }
  | { action: "stop" }
  | { action: "input"; input: BrowserSurfaceInput }

export interface SemanticNode {
  ref: string
  role: string
  name: string
  value?: string
  description?: string
  disabled?: boolean
  focused?: boolean
}

export interface BrowserDownload {
  guid: string
  state: "inProgress" | "completed" | "canceled"
  filename: string
  receivedBytes: number
  totalBytes: number
  path?: string
  errorCode?: string
}

export interface ToolResultEnvelope {
  ok: boolean
  action: string
  sessionId: string
  tab?: BrowserTab
  data?: unknown
  error?: {
    code: string
    message: string
    retryable: boolean
    recovery?: string
  }
}

export interface BrowserToolOutput {
  envelope: ToolResultEnvelope
  image?: {
    data: string
    mimeType: "image/png" | "image/jpeg"
  }
}

export interface PageSnapshot {
  nodes: SemanticNode[]
  documentUrl: string
  title: string
}

export interface PageController {
  info(): Promise<BrowserTab>
  navigate(url: string, timeoutMs: number): Promise<BrowserTab>
  snapshot(options: {
    interactive: boolean
    limit: number
    timeoutMs: number
  }): Promise<PageSnapshot>
  screenshot(options: {
    format: "png" | "jpeg"
    fullPage: boolean
    quality?: number
    timeoutMs: number
  }): Promise<{ data: string; mimeType: "image/png" | "image/jpeg" }>
  click(ref: string, timeoutMs: number): Promise<void>
  type(
    ref: string,
    text: string,
    clear: boolean,
    timeoutMs: number
  ): Promise<void>
  press(key: string, timeoutMs: number): Promise<void>
  scroll(
    ref: string | undefined,
    deltaX: number,
    deltaY: number,
    timeoutMs: number
  ): Promise<void>
  wait(options: {
    milliseconds?: number
    text?: string
    urlIncludes?: string
    timeoutMs: number
  }): Promise<void>
  surfaceState(): Promise<BrowserSurfacePageState>
  goHistory(delta: -1 | 1, timeoutMs: number): Promise<void>
  reload(timeoutMs: number): Promise<void>
  stopLoading(timeoutMs: number): Promise<void>
  dispatchSurfaceInput(
    input: BrowserSurfaceInput,
    timeoutMs: number
  ): Promise<void>
  setSurfaceViewport(
    width: number,
    height: number,
    timeoutMs: number
  ): Promise<void>
  startScreencast(
    listener: (frame: BrowserSurfaceFrame) => void
  ): Promise<() => Promise<void>>
  close(): Promise<void>
}

export interface BrowserBackend {
  readonly state: BrowserRuntimeState
  status(): BrowserRuntimeStatus
  doctor(): Promise<BrowserDoctorResult>
  start(): Promise<void>
  stop(): Promise<void>
  shutdown(): Promise<void>
  recover(): Promise<void>
  sessionSnapshot(
    sessionId: string,
    ensure: boolean
  ): Promise<BrowserBackendSessionSnapshot | null>
  listTargets(sessionId: string): Promise<BrowserTab[]>
  openTarget(sessionId: string, url?: string): Promise<PageController>
  page(sessionId: string, targetId: string): Promise<PageController>
  focusTarget(sessionId: string, targetId: string): Promise<void>
  closeTarget(sessionId: string, targetId: string): Promise<void>
  downloads(sessionId: string): Promise<BrowserDownload[]>
  waitForDownload(
    sessionId: string,
    guid: string | undefined,
    timeoutMs: number
  ): Promise<BrowserDownload>
  assignTarget(targetId: string, sessionId: string): void
  releaseSession(sessionId: string): Promise<void>
}
