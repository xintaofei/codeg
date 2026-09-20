import { act, cleanup, render, waitFor } from "@testing-library/react"
import { NextIntlClientProvider } from "next-intl"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import type { ComposerInjectContent } from "@/components/chat/message-input"
import type { PromptDraft } from "@/lib/types"
import { useConversationRuntimeStore } from "@/stores/conversation-runtime-store"
import { useAppWorkspaceStore } from "@/stores/app-workspace-store"
import { CanvasConversationSurface } from "./canvas-conversation-surface"

const toastMock = vi.hoisted(() => ({
  error: vi.fn(),
  warning: vi.fn(),
  info: vi.fn(),
  success: vi.fn(),
}))
vi.mock("sonner", () => ({
  toast: toastMock,
}))

const apiMocks = vi.hoisted(() => ({
  createConversation: vi.fn(),
  createChatConversation: vi.fn(),
  createChatDir: vi.fn(async () => ({ path: "/mock/chat/dir" })),
  acpStopAsyncTask: vi.fn<
    (connectionId: string, taskId: string) => Promise<boolean>
  >(async () => true),
}))
vi.mock("@/lib/api", async (importOriginal) => {
  const actual = await importOriginal<typeof import("@/lib/api")>()
  return {
    ...actual,
    createConversation: (folderId: number, agentType: string, title?: string) =>
      apiMocks.createConversation(folderId, agentType, title),
    createChatConversation: (
      agentType: string,
      title?: string,
      workingDir?: string
    ) => apiMocks.createChatConversation(agentType, title, workingDir),
    createChatDir: () => apiMocks.createChatDir(),
    acpStopAsyncTask: (connectionId: string, taskId: string) =>
      apiMocks.acpStopAsyncTask(connectionId, taskId),
  }
})

const lifecycleSendMock = vi.hoisted(() => vi.fn())
vi.mock("@/hooks/use-connection-lifecycle", () => ({
  useConnectionLifecycle: () => ({
    conn: {
      status: "connected",
      promptCapabilities: {
        image: true,
        audio: false,
        embedded_context: true,
      },
      connectionId: "conn-test",
      sessionId: "session-test",
      modes: {
        current_mode_id: "default",
        available_modes: [{ id: "default", name: "Default" }],
      },
      configOptions: [],
      availableCommands: [],
      error: null,
      claudeApiRetry: null,
      sessionFailures: [],
      asyncTasks: [],
      pendingPermission: null,
      pendingQuestion: null,
      pendingAskQuestion: null,
      pendingPlanApproval: null,
    },
    modeLoading: false,
    configOptionsLoading: false,
    selectorsLoading: false,
    autoConnectError: null,
    handleFocus: vi.fn(),
    handleSend: lifecycleSendMock,
    handleSetConfigOption: vi.fn(),
    handleCancel: vi.fn(),
    handleRespondPermission: vi.fn(),
  }),
}))

vi.mock("@/hooks/use-conversation-detail", () => ({
  useConversationDetail: () => ({
    detail: null,
    loading: false,
    error: null,
    acpLoadError: null,
  }),
}))

vi.mock("@/contexts/acp-connections-context", () => ({
  useAcpActions: () => ({
    registerLiveMessageSink: vi.fn(() => () => {}),
    answerQuestion: vi.fn(),
    answerPlanApproval: vi.fn(),
  }),
}))

vi.mock("@/components/message/message-list-view", () => ({
  MessageListView: () => <div data-testid="message-list-view" />,
}))

vi.mock("@/components/chat/agent-selector", () => ({
  AgentSelector: () => <div data-testid="agent-selector" />,
}))

interface CapturedInputProps {
  onSend: (draft: PromptDraft, modeId?: string | null) => boolean | void
  injectContent?: ComposerInjectContent | null
  onInjectConsumed?: () => void
}

const capturedMessageInputProps = vi.hoisted(() => ({
  current: null as CapturedInputProps | null,
}))

vi.mock("@/components/chat/message-input", () => ({
  MessageInput: (props: CapturedInputProps) => {
    capturedMessageInputProps.current = props
    return (
      <div data-testid="mock-message-input">
        {props.injectContent && (
          <div data-testid="mock-injected-content">
            {props.injectContent.text} ({props.injectContent.mode})
          </div>
        )}
      </div>
    )
  },
}))

function renderSurface(
  props: Partial<React.ComponentProps<typeof CanvasConversationSurface>> = {}
) {
  const defaultProps = {
    contextKey: "test-card-context-key",
    conversationId: null,
    agentType: "claude" as const,
    draftTarget: {
      kind: "folder" as const,
      folderId: 1,
      workingDir: "/test/dir",
    },
    isActive: true,
  }
  return render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <CanvasConversationSurface {...defaultProps} {...props} />
    </NextIntlClientProvider>
  )
}

describe("CanvasConversationSurface", () => {
  beforeEach(() => {
    vi.clearAllMocks()
    capturedMessageInputProps.current = null
    useConversationRuntimeStore.setState({
      byConversationId: new Map(),
    })
    useAppWorkspaceStore.setState({
      folders: [],
      allFolders: [],
      conversations: [],
    })
  })

  afterEach(() => {
    cleanup()
  })

  describe("creation failure handling", () => {
    it("injects content back into MessageInput with mode append and shows toast.error when createConversation fails", async () => {
      apiMocks.createConversation.mockRejectedValueOnce(
        new Error("Database write error")
      )

      renderSurface({
        draftTarget: {
          kind: "folder",
          folderId: 10,
          workingDir: "/workspace/proj",
        },
      })

      await waitFor(() =>
        expect(capturedMessageInputProps.current).toBeTruthy()
      )

      const draft: PromptDraft = {
        displayText: "Please fix the login timeout issue",
        blocks: [{ type: "text", text: "Please fix the login timeout issue" }],
      }

      act(() => {
        capturedMessageInputProps.current?.onSend(draft)
      })

      await waitFor(() => {
        expect(toastMock.error).toHaveBeenCalledWith(
          enMessages.Canvas.createFailed
        )
      })

      expect(capturedMessageInputProps.current?.injectContent).toEqual({
        text: "Please fix the login timeout issue",
        mode: "append",
      })
    })

    it("injects content back into MessageInput with mode append and shows toast.error when createChatConversation fails", async () => {
      apiMocks.createChatConversation.mockRejectedValueOnce(
        new Error("Chat creation failure")
      )

      renderSurface({
        draftTarget: {
          kind: "chat",
        },
      })

      await waitFor(() =>
        expect(capturedMessageInputProps.current).toBeTruthy()
      )

      const draft: PromptDraft = {
        displayText: "How do I use Docker compose?",
        blocks: [{ type: "text", text: "How do I use Docker compose?" }],
      }

      act(() => {
        capturedMessageInputProps.current?.onSend(draft)
      })

      await waitFor(() => {
        expect(toastMock.error).toHaveBeenCalledWith(
          enMessages.Canvas.createFailed
        )
      })

      expect(capturedMessageInputProps.current?.injectContent).toEqual({
        text: "How do I use Docker compose?",
        mode: "append",
      })
    })
  })

  describe("in-flight creation concurrency", () => {
    it("returns false from second onSend while creation is in-flight and does not lose text or duplicate creation", async () => {
      let resolveCreation!: (val: number) => void
      apiMocks.createConversation.mockReturnValueOnce(
        new Promise<number>((resolve) => {
          resolveCreation = resolve
        })
      )

      renderSurface({
        draftTarget: {
          kind: "folder",
          folderId: 10,
          workingDir: "/workspace/proj",
        },
      })

      await waitFor(() =>
        expect(capturedMessageInputProps.current).toBeTruthy()
      )

      const firstDraft: PromptDraft = {
        displayText: "First prompt in flight",
        blocks: [{ type: "text", text: "First prompt in flight" }],
      }
      const secondDraft: PromptDraft = {
        displayText: "Second prompt while creating",
        blocks: [{ type: "text", text: "Second prompt while creating" }],
      }

      // First send initiates creation
      let firstResult: boolean | void = undefined
      act(() => {
        firstResult = capturedMessageInputProps.current?.onSend(firstDraft)
      })
      expect(firstResult).not.toBe(false)
      expect(apiMocks.createConversation).toHaveBeenCalledTimes(1)

      // Second send while creation is still in-flight
      let secondResult: boolean | void = undefined
      act(() => {
        secondResult = capturedMessageInputProps.current?.onSend(secondDraft)
      })

      // Second send must return false to signal MessageInput not to clear draft
      expect(secondResult).toBe(false)
      // Must not start a second conversation creation
      expect(apiMocks.createConversation).toHaveBeenCalledTimes(1)

      // Resolve the first creation cleanly
      await act(async () => {
        resolveCreation(42)
      })

      // Verify lifecycleSend was only called for the first draft
      expect(lifecycleSendMock).toHaveBeenCalledTimes(1)
      expect(lifecycleSendMock).toHaveBeenCalledWith(
        firstDraft,
        undefined,
        expect.objectContaining({ conversationId: 42, folderId: 10 })
      )
    })
  })
})
