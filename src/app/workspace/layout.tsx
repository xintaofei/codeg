"use client"

import {
  Suspense,
  useMemo,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react"
import type { ImperativePanelGroupHandle } from "react-resizable-panels"
import { FolderTitleBar } from "@/components/layout/folder-title-bar"
import { Sidebar } from "@/components/layout/sidebar"
import { StatusBar } from "@/components/layout/status-bar"
import {
  AppWorkspaceProvider,
  ConversationStatusEventBridge,
} from "@/contexts/app-workspace-context"
import {
  ActiveFolderProvider,
  useActiveFolder,
} from "@/contexts/active-folder-context"
import { TaskProvider } from "@/contexts/task-context"
import { AlertProvider } from "@/contexts/alert-context"
import {
  AcpConnectionsProvider,
  useAcpActions,
} from "@/contexts/acp-connections-context"
import { DelegationProvider } from "@/contexts/delegation-context"
import { ConversationRuntimeProvider } from "@/contexts/conversation-runtime-context"
import { TabProvider, useTabContext } from "@/contexts/tab-context"
import { SessionStatsProvider } from "@/contexts/session-stats-context"
import { SidebarProvider, useSidebarContext } from "@/contexts/sidebar-context"
import {
  AuxPanelProvider,
  useAuxPanelContext,
} from "@/contexts/aux-panel-context"
import {
  TerminalProvider,
  useTerminalContext,
} from "@/contexts/terminal-context"
import { GitCredentialProvider } from "@/contexts/git-credential-context"
import {
  WorkspaceProvider,
  useWorkspaceContext,
} from "@/contexts/workspace-context"
import { RemoteConnectionGate } from "@/contexts/remote-connection-context"
import { UpdateProvider } from "@/components/providers/update-provider"
import { TabBar } from "@/components/tabs/tab-bar"
import { TerminalPanel } from "@/components/terminal/terminal-panel"
import { AuxPanel } from "@/components/layout/aux-panel"
import { FileWorkspaceTabBar } from "@/components/files/file-workspace-tab-bar"
import { FileWorkspacePanel } from "@/components/files/file-workspace-panel"
import { AppToaster } from "@/components/ui/app-toaster"
import {
  DeepLinkBootstrap,
  PetFocusBridge,
} from "@/components/workspace/deep-link-bootstrap"
import { WorkspaceOpenFolderListener } from "@/components/workspace/workspace-open-folder-listener"
import {
  ResizableHandle,
  ResizablePanel,
  ResizablePanelGroup,
} from "@/components/ui/resizable"
import { cn } from "@/lib/utils"
import { useIsMobile } from "@/hooks/use-mobile"
import { Sheet, SheetContent, SheetTitle } from "@/components/ui/sheet"

function WorkspaceDocumentTitle() {
  const { activeFolder } = useActiveFolder()

  useEffect(() => {
    document.title = activeFolder ? `${activeFolder.name} - codeg` : "codeg"
  }, [activeFolder])

  return null
}

const TOAST_DURATION_MS = 15000
const WORKSPACE_PANEL_GROUP_ID = "workspace-panel-group"
const WORKSPACE_CONVERSATION_PANEL_ID = "workspace-conversation-panel"
const WORKSPACE_FILES_PANEL_ID = "workspace-files-panel"
const FOLDER_SHELL_GROUP_ID = "folder-shell-group"
const FOLDER_SHELL_LEFT_PANEL_ID = "folder-shell-left-panel"
const FOLDER_SHELL_MAIN_PANEL_ID = "folder-shell-main-panel"
const FOLDER_SHELL_RIGHT_PANEL_ID = "folder-shell-right-panel"
const FOLDER_MAIN_GROUP_ID = "folder-main-group"
const FOLDER_MAIN_WORKSPACE_PANEL_ID = "folder-main-workspace-panel"
const FOLDER_MAIN_TERMINAL_PANEL_ID = "folder-main-terminal-panel"
const DEFAULT_FUSION_LAYOUT: [number, number] = [56, 44]
const MIN_CENTER_WIDTH_PX = 420
const MIN_WORKSPACE_HEIGHT_PX = 220
const LAYOUT_EPSILON = 0.25

function TabKeysSync() {
  const { tabs } = useTabContext()
  const { registerOpenTabKeys } = useAcpActions()
  const keys = useMemo(() => new Set(tabs.map((t) => t.id)), [tabs])
  useEffect(() => {
    registerOpenTabKeys(keys)
  }, [keys, registerOpenTabKeys])
  return null
}

function isSameLayout(a: number[], b: number[]): boolean {
  if (a.length !== b.length) return false
  return a.every((value, index) => Math.abs(value - b[index]) <= LAYOUT_EPSILON)
}

function clamp(value: number, min: number, max: number): number {
  return Math.max(min, Math.min(max, value))
}

function toPercent(pixels: number, totalPixels: number): number {
  if (totalPixels <= 0) return 0
  return (pixels / totalPixels) * 100
}

function resolvePanelSizeRange(
  minPixels: number,
  maxPixels: number,
  totalPixels: number
): { minSize: number; maxSize: number } {
  const safeTotal = totalPixels > 0 ? totalPixels : 1
  const minSize = clamp(toPercent(minPixels, safeTotal), 0, 100)
  const maxSize = clamp(toPercent(maxPixels, safeTotal), minSize, 100)
  return { minSize, maxSize }
}

function WorkspaceContent({ children }: { children: React.ReactNode }) {
  const { mode, layoutMode, setActivePane, filesMaximized } =
    useWorkspaceContext()
  const panelGroupRef = useRef<ImperativePanelGroupHandle | null>(null)
  const fusionLayoutRef = useRef<[number, number]>(DEFAULT_FUSION_LAYOUT)
  const desiredLayoutRef = useRef<[number, number]>(DEFAULT_FUSION_LAYOUT)
  const appliedLayoutRef = useRef<[number, number] | null>(null)

  const markConversationActive = useCallback(() => {
    if (mode !== "fusion" || filesMaximized) return
    setActivePane("conversation")
  }, [mode, filesMaximized, setActivePane])

  const markFileActive = useCallback(() => {
    if (mode !== "fusion") return
    setActivePane("files")
  }, [mode, setActivePane])

  const applyLayout = useCallback((layout: [number, number]) => {
    desiredLayoutRef.current = layout
    if (
      appliedLayoutRef.current &&
      isSameLayout(appliedLayoutRef.current, layout)
    ) {
      return
    }

    const panelGroup = panelGroupRef.current
    if (!panelGroup) return

    try {
      panelGroup.setLayout(layout)
      appliedLayoutRef.current = layout
    } catch {
      /* retry via onLayout */
    }
  }, [])

  useEffect(() => {
    if (mode === "fusion" && layoutMode === "fusion") {
      applyLayout(fusionLayoutRef.current)
    }
  }, [applyLayout, mode, layoutMode])

  const handleLayout = useCallback(
    (layout: number[]) => {
      if (layout.length !== 2) return

      const normalizedLayout: [number, number] = [layout[0], layout[1]]
      appliedLayoutRef.current = normalizedLayout

      const desired = desiredLayoutRef.current
      if (mode !== "fusion" && !isSameLayout(normalizedLayout, desired)) {
        applyLayout(desired)
        return
      }

      if (mode !== "fusion") return

      const [conversationSize, fileSize] = normalizedLayout
      if (conversationSize <= 0 || fileSize <= 0) return
      fusionLayoutRef.current = [conversationSize, fileSize]
    },
    [applyLayout, mode]
  )

  if (layoutMode === "files" && mode === "fusion") {
    return <SinglePaneWorkspaceContent>{children}</SinglePaneWorkspaceContent>
  }

  return (
    <div className="relative h-full min-h-0 overflow-hidden">
      <ResizablePanelGroup
        id={WORKSPACE_PANEL_GROUP_ID}
        ref={panelGroupRef}
        direction="horizontal"
        onLayout={handleLayout}
      >
        <ResizablePanel
          id={WORKSPACE_CONVERSATION_PANEL_ID}
          order={1}
          defaultSize={56}
          minSize={mode === "fusion" ? 25 : 0}
        >
          <section
            className={cn(
              "flex h-full min-h-0 flex-col overflow-hidden",
              mode === "conversation" && "absolute inset-0 z-30 bg-background"
            )}
            onPointerDownCapture={markConversationActive}
            onFocusCapture={markConversationActive}
            inert={filesMaximized || undefined}
          >
            <TabBar />
            <div className="relative flex-1 min-h-0 overflow-hidden">
              {children}
            </div>
          </section>
        </ResizablePanel>
        <ResizableHandle
          withHandle
          disabled={mode !== "fusion"}
          className={
            mode === "fusion"
              ? ""
              : "pointer-events-none w-0 opacity-0 after:w-0"
          }
        />
        <ResizablePanel
          id={WORKSPACE_FILES_PANEL_ID}
          order={2}
          defaultSize={44}
          minSize={mode === "fusion" ? 20 : 0}
        >
          {/* When maximized, overlay the file section across the entire
              workspace area instead of resizing the conversation panel — that
              would fire ResizeObserver on the conversation's stick-to-bottom
              scroll container and reset its position.
              The `absolute inset-0` resolves to the outer `relative` wrapper,
              not the Panel root. This depends on react-resizable-panels
              keeping the Panel root at `position: static`; if a future
              version sets `position: relative` there, this overlay (and the
              mirrored `mode === "conversation"` overlay above) would clip to
              the Panel's allocated slice and need to be lifted outside the
              panel group. */}
          <section
            className={cn(
              "flex h-full min-h-0 flex-col overflow-hidden",
              filesMaximized && "absolute inset-0 z-30 bg-background"
            )}
            onPointerDownCapture={markFileActive}
            onFocusCapture={markFileActive}
            aria-hidden={mode === "conversation"}
          >
            <FileWorkspaceTabBar />
            <div className="flex-1 min-h-0 overflow-hidden">
              <FileWorkspacePanel />
            </div>
          </section>
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  )
}

function MobileWorkspaceContent({ children }: { children: React.ReactNode }) {
  const { mode, activePane } = useWorkspaceContext()

  const showConversation =
    mode === "conversation" || activePane === "conversation"

  return (
    <div className="relative h-full min-h-0 overflow-hidden">
      {showConversation ? (
        <section className="flex h-full min-h-0 flex-col overflow-hidden">
          <TabBar />
          <div className="relative flex-1 min-h-0 overflow-hidden">
            {children}
          </div>
        </section>
      ) : (
        <section className="flex h-full min-h-0 flex-col overflow-hidden">
          <FileWorkspaceTabBar />
          <div className="flex-1 min-h-0 overflow-hidden">
            <FileWorkspacePanel />
          </div>
        </section>
      )}
    </div>
  )
}

const SinglePaneWorkspaceContent = MobileWorkspaceContent

function MobileFolderWorkspaceShell({
  children,
}: {
  children: React.ReactNode
}) {
  const {
    isOpen: sidebarOpen,
    restored: sidebarRestored,
    toggle: toggleSidebar,
  } = useSidebarContext()
  const {
    isOpen: auxOpen,
    restored: auxRestored,
    toggle: toggleAux,
  } = useAuxPanelContext()
  const { isOpen: terminalOpen, toggle: toggleTerminal } = useTerminalContext()

  return (
    <div className="flex flex-1 min-h-0 overflow-hidden">
      <Sheet open={sidebarRestored && sidebarOpen} onOpenChange={toggleSidebar}>
        <SheetContent
          side="left"
          showCloseButton={false}
          className="w-[85%] max-w-[360px] p-0"
        >
          <SheetTitle className="sr-only">Sidebar</SheetTitle>
          <Sidebar />
        </SheetContent>
      </Sheet>

      <main className="flex h-full min-h-0 w-full flex-col overflow-hidden">
        <MobileWorkspaceContent>{children}</MobileWorkspaceContent>
      </main>

      <Sheet open={auxRestored && auxOpen} onOpenChange={toggleAux}>
        <SheetContent
          side="right"
          showCloseButton={false}
          className="w-[85%] max-w-[360px] p-0"
        >
          <SheetTitle className="sr-only">Panel</SheetTitle>
          <AuxPanel />
        </SheetContent>
      </Sheet>

      <Sheet open={terminalOpen} onOpenChange={toggleTerminal}>
        <SheetContent
          side="bottom"
          showCloseButton={false}
          className="!h-[70vh] p-0"
        >
          <SheetTitle className="sr-only">Terminal</SheetTitle>
          <div className="h-full min-h-0 overflow-hidden">
            <TerminalPanel />
          </div>
        </SheetContent>
      </Sheet>
    </div>
  )
}

function FolderWorkspaceShell({ children }: { children: React.ReactNode }) {
  const {
    isOpen: sidebarOpen,
    width: sidebarWidth,
    minWidth: sidebarMinWidth,
    maxWidth: sidebarMaxWidth,
    setWidth: setSidebarWidth,
  } = useSidebarContext()
  const {
    isOpen: auxOpen,
    width: auxWidth,
    minWidth: auxMinWidth,
    maxWidth: auxMaxWidth,
    setWidth: setAuxWidth,
  } = useAuxPanelContext()
  const {
    isOpen: terminalOpen,
    height: terminalHeight,
    minHeight: terminalMinHeight,
    maxHeight: terminalMaxHeight,
    setHeight: setTerminalHeight,
  } = useTerminalContext()

  const shellGroupRef = useRef<ImperativePanelGroupHandle | null>(null)
  const mainGroupRef = useRef<ImperativePanelGroupHandle | null>(null)
  const shellContainerRef = useRef<HTMLDivElement | null>(null)
  const mainContainerRef = useRef<HTMLDivElement | null>(null)

  const [shellWidth, setShellWidth] = useState(0)
  const [mainHeight, setMainHeight] = useState(0)

  const shellDesiredLayoutRef = useRef<[number, number, number]>([0, 100, 0])
  const shellAppliedLayoutRef = useRef<[number, number, number] | null>(null)
  const mainDesiredLayoutRef = useRef<[number, number]>([100, 0])
  const mainAppliedLayoutRef = useRef<[number, number] | null>(null)

  useEffect(() => {
    const container = shellContainerRef.current
    if (!container) return

    const updateWidth = (next: number) => {
      setShellWidth((prev) => (Math.abs(prev - next) < 1 ? prev : next))
    }

    updateWidth(container.clientWidth)
    const observer = new ResizeObserver((entries) => {
      updateWidth(entries[0]?.contentRect.width ?? container.clientWidth)
    })

    observer.observe(container)
    return () => {
      observer.disconnect()
    }
  }, [])

  useEffect(() => {
    const container = mainContainerRef.current
    if (!container) return

    const updateHeight = (next: number) => {
      setMainHeight((prev) => (Math.abs(prev - next) < 1 ? prev : next))
    }

    updateHeight(container.clientHeight)
    const observer = new ResizeObserver((entries) => {
      updateHeight(entries[0]?.contentRect.height ?? container.clientHeight)
    })

    observer.observe(container)
    return () => {
      observer.disconnect()
    }
  }, [])

  const buildShellLayout = useCallback((): [number, number, number] => {
    const requestedLeft = sidebarOpen
      ? clamp(sidebarWidth, sidebarMinWidth, sidebarMaxWidth)
      : 0
    const requestedRight = auxOpen
      ? clamp(auxWidth, auxMinWidth, auxMaxWidth)
      : 0

    const totalWidth =
      shellWidth > 0 ? shellWidth : requestedLeft + requestedRight + 960

    let left = requestedLeft
    let right = requestedRight

    const maxSideTotal = Math.max(0, totalWidth - MIN_CENTER_WIDTH_PX)
    const sideTotal = left + right
    if (sideTotal > maxSideTotal && sideTotal > 0) {
      const scale = maxSideTotal / sideTotal
      left *= scale
      right *= scale
    }

    const center = Math.max(1, totalWidth - left - right)
    const total = left + center + right

    return [(left / total) * 100, (center / total) * 100, (right / total) * 100]
  }, [
    auxMaxWidth,
    auxMinWidth,
    auxOpen,
    auxWidth,
    shellWidth,
    sidebarMaxWidth,
    sidebarMinWidth,
    sidebarOpen,
    sidebarWidth,
  ])

  const buildMainLayout = useCallback((): [number, number] => {
    if (!terminalOpen) {
      return [100, 0]
    }

    const requestedTerminalHeight = clamp(
      terminalHeight,
      terminalMinHeight,
      terminalMaxHeight
    )
    const totalHeight =
      mainHeight > 0 ? mainHeight : requestedTerminalHeight + 640

    const maxTerminalHeight = Math.max(0, totalHeight - MIN_WORKSPACE_HEIGHT_PX)
    const terminal = Math.min(requestedTerminalHeight, maxTerminalHeight)
    const workspace = Math.max(1, totalHeight - terminal)
    const total = workspace + terminal

    return [(workspace / total) * 100, (terminal / total) * 100]
  }, [
    mainHeight,
    terminalHeight,
    terminalMaxHeight,
    terminalMinHeight,
    terminalOpen,
  ])

  const applyShellLayout = useCallback((layout: [number, number, number]) => {
    shellDesiredLayoutRef.current = layout
    if (
      shellAppliedLayoutRef.current &&
      isSameLayout(shellAppliedLayoutRef.current, layout)
    ) {
      return
    }

    const shellGroup = shellGroupRef.current
    if (!shellGroup) return

    try {
      shellGroup.setLayout(layout)
      shellAppliedLayoutRef.current = layout
    } catch {
      /* retry via onLayout */
    }
  }, [])

  const applyMainLayout = useCallback((layout: [number, number]) => {
    mainDesiredLayoutRef.current = layout
    if (
      mainAppliedLayoutRef.current &&
      isSameLayout(mainAppliedLayoutRef.current, layout)
    ) {
      return
    }

    const mainGroup = mainGroupRef.current
    if (!mainGroup) return

    try {
      mainGroup.setLayout(layout)
      mainAppliedLayoutRef.current = layout
    } catch {
      /* retry via onLayout */
    }
  }, [])

  useEffect(() => {
    applyShellLayout(buildShellLayout())
  }, [applyShellLayout, buildShellLayout])

  useEffect(() => {
    applyMainLayout(buildMainLayout())
  }, [applyMainLayout, buildMainLayout])

  const handleShellLayout = useCallback(
    (layout: number[]) => {
      if (layout.length !== 3) return

      const normalizedLayout: [number, number, number] = [
        layout[0],
        layout[1],
        layout[2],
      ]
      shellAppliedLayoutRef.current = normalizedLayout

      const desired = shellDesiredLayoutRef.current
      const shouldEnforceDesiredLayout =
        (!sidebarOpen && normalizedLayout[0] > LAYOUT_EPSILON) ||
        (!auxOpen && normalizedLayout[2] > LAYOUT_EPSILON)

      if (
        shouldEnforceDesiredLayout &&
        !isSameLayout(normalizedLayout, desired)
      ) {
        applyShellLayout(desired)
        return
      }

      if (shellWidth <= 0) return

      if (sidebarOpen) {
        const nextSidebarWidth = (normalizedLayout[0] / 100) * shellWidth
        const withinSidebarRange =
          nextSidebarWidth >= sidebarMinWidth - 1 &&
          nextSidebarWidth <= sidebarMaxWidth + 1
        if (
          withinSidebarRange &&
          Math.abs(nextSidebarWidth - sidebarWidth) >= 1
        ) {
          setSidebarWidth(nextSidebarWidth)
        }
      }

      if (auxOpen) {
        const nextAuxWidth = (normalizedLayout[2] / 100) * shellWidth
        const withinAuxRange =
          nextAuxWidth >= auxMinWidth - 1 && nextAuxWidth <= auxMaxWidth + 1
        if (withinAuxRange && Math.abs(nextAuxWidth - auxWidth) >= 1) {
          setAuxWidth(nextAuxWidth)
        }
      }
    },
    [
      applyShellLayout,
      auxMaxWidth,
      auxMinWidth,
      auxOpen,
      auxWidth,
      setAuxWidth,
      setSidebarWidth,
      shellWidth,
      sidebarMaxWidth,
      sidebarMinWidth,
      sidebarOpen,
      sidebarWidth,
    ]
  )

  const handleMainLayout = useCallback(
    (layout: number[]) => {
      if (layout.length !== 2) return

      const normalizedLayout: [number, number] = [layout[0], layout[1]]
      mainAppliedLayoutRef.current = normalizedLayout

      const desired = mainDesiredLayoutRef.current
      if (
        !terminalOpen &&
        normalizedLayout[1] > LAYOUT_EPSILON &&
        !isSameLayout(normalizedLayout, desired)
      ) {
        applyMainLayout(desired)
        return
      }

      if (!terminalOpen || mainHeight <= 0) return

      const nextTerminalHeight = (normalizedLayout[1] / 100) * mainHeight
      const withinTerminalRange =
        nextTerminalHeight >= terminalMinHeight - 1 &&
        nextTerminalHeight <= terminalMaxHeight + 1
      if (
        withinTerminalRange &&
        Math.abs(nextTerminalHeight - terminalHeight) >= 1
      ) {
        setTerminalHeight(nextTerminalHeight)
      }
    },
    [
      applyMainLayout,
      mainHeight,
      setTerminalHeight,
      terminalHeight,
      terminalMaxHeight,
      terminalMinHeight,
      terminalOpen,
    ]
  )

  const safeShellWidth = shellWidth > 0 ? shellWidth : 1440
  const sidebarSizeRange = resolvePanelSizeRange(
    sidebarMinWidth,
    sidebarMaxWidth,
    safeShellWidth
  )
  const auxSizeRange = resolvePanelSizeRange(
    auxMinWidth,
    auxMaxWidth,
    safeShellWidth
  )

  const safeMainHeight = mainHeight > 0 ? mainHeight : 900
  const terminalSizeRange = resolvePanelSizeRange(
    terminalMinHeight,
    terminalMaxHeight,
    safeMainHeight
  )

  return (
    <div
      ref={shellContainerRef}
      className="flex flex-1 min-h-0 overflow-hidden"
    >
      <ResizablePanelGroup
        id={FOLDER_SHELL_GROUP_ID}
        ref={shellGroupRef}
        direction="horizontal"
        onLayout={handleShellLayout}
      >
        <ResizablePanel
          id={FOLDER_SHELL_LEFT_PANEL_ID}
          order={1}
          defaultSize={18}
          minSize={sidebarOpen ? sidebarSizeRange.minSize : 0}
          maxSize={sidebarOpen ? sidebarSizeRange.maxSize : 0}
        >
          <div className="h-full min-h-0 overflow-hidden">
            <Sidebar />
          </div>
        </ResizablePanel>

        <ResizableHandle
          withHandle
          disabled={!sidebarOpen}
          className={
            sidebarOpen ? "" : "pointer-events-none w-0 opacity-0 after:w-0"
          }
        />

        <ResizablePanel
          id={FOLDER_SHELL_MAIN_PANEL_ID}
          order={2}
          defaultSize={64}
          minSize={10}
        >
          <main
            ref={mainContainerRef}
            className="flex h-full min-h-0 flex-col overflow-hidden"
          >
            <ResizablePanelGroup
              id={FOLDER_MAIN_GROUP_ID}
              ref={mainGroupRef}
              direction="vertical"
              onLayout={handleMainLayout}
            >
              <ResizablePanel
                id={FOLDER_MAIN_WORKSPACE_PANEL_ID}
                order={1}
                defaultSize={72}
                minSize={15}
              >
                <WorkspaceContent>{children}</WorkspaceContent>
              </ResizablePanel>

              <ResizableHandle
                withHandle
                disabled={!terminalOpen}
                className={
                  terminalOpen
                    ? ""
                    : "pointer-events-none h-0 opacity-0 after:h-0"
                }
              />

              <ResizablePanel
                id={FOLDER_MAIN_TERMINAL_PANEL_ID}
                order={2}
                defaultSize={28}
                minSize={terminalOpen ? terminalSizeRange.minSize : 0}
                maxSize={terminalOpen ? terminalSizeRange.maxSize : 0}
              >
                <div className="h-full min-h-0 overflow-hidden">
                  <TerminalPanel />
                </div>
              </ResizablePanel>
            </ResizablePanelGroup>
          </main>
        </ResizablePanel>

        <ResizableHandle
          withHandle
          disabled={!auxOpen}
          className={
            auxOpen ? "" : "pointer-events-none w-0 opacity-0 after:w-0"
          }
        />

        <ResizablePanel
          id={FOLDER_SHELL_RIGHT_PANEL_ID}
          order={3}
          defaultSize={18}
          minSize={auxOpen ? auxSizeRange.minSize : 0}
          maxSize={auxOpen ? auxSizeRange.maxSize : 0}
        >
          <div className="h-full min-h-0 overflow-hidden">
            <AuxPanel />
          </div>
        </ResizablePanel>
      </ResizablePanelGroup>
    </div>
  )
}

function FolderLayoutShell({ children }: { children: React.ReactNode }) {
  const isMobile = useIsMobile()

  return (
    <div className="fixed inset-0 flex flex-col overflow-hidden bg-background text-foreground pt-[env(safe-area-inset-top)] pr-[env(safe-area-inset-right)] pb-[env(safe-area-inset-bottom)] pl-[env(safe-area-inset-left)]">
      <FolderTitleBar />
      {isMobile ? (
        <MobileFolderWorkspaceShell>{children}</MobileFolderWorkspaceShell>
      ) : (
        <FolderWorkspaceShell>{children}</FolderWorkspaceShell>
      )}
      <StatusBar />
      <AppToaster
        position="bottom-right"
        duration={TOAST_DURATION_MS}
        closeButton
      />
    </div>
  )
}

function WorkspaceLayoutInner({ children }: { children: React.ReactNode }) {
  return (
    <AppWorkspaceProvider>
      <ActiveFolderProvider>
        <AlertProvider>
          <GitCredentialProvider>
            <TaskProvider>
              <AcpConnectionsProvider>
                <DelegationProvider>
                  <ConversationStatusEventBridge />
                  <ConversationRuntimeProvider>
                    <WorkspaceProvider>
                      <TabProvider>
                        <WorkspaceDocumentTitle />
                        <TabKeysSync />
                        <DeepLinkBootstrap />
                        <PetFocusBridge />
                        <WorkspaceOpenFolderListener />
                        <SessionStatsProvider>
                          <SidebarProvider>
                            <AuxPanelProvider>
                              <TerminalProvider>
                                <FolderLayoutShell>
                                  {children}
                                </FolderLayoutShell>
                              </TerminalProvider>
                            </AuxPanelProvider>
                          </SidebarProvider>
                        </SessionStatsProvider>
                      </TabProvider>
                    </WorkspaceProvider>
                  </ConversationRuntimeProvider>
                </DelegationProvider>
              </AcpConnectionsProvider>
            </TaskProvider>
          </GitCredentialProvider>
        </AlertProvider>
      </ActiveFolderProvider>
    </AppWorkspaceProvider>
  )
}

export default function WorkspaceLayout({
  children,
}: {
  children: React.ReactNode
}) {
  return (
    <Suspense>
      <RemoteConnectionGate>
        <UpdateProvider>
          <WorkspaceLayoutInner>{children}</WorkspaceLayoutInner>
        </UpdateProvider>
      </RemoteConnectionGate>
    </Suspense>
  )
}
