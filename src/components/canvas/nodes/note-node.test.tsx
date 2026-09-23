import { fireEvent, render, screen } from "@testing-library/react"
import { ReactFlowProvider } from "@xyflow/react"
import { NextIntlClientProvider } from "next-intl"
import { beforeEach, describe, expect, it, vi } from "vitest"
import enMessages from "@/i18n/messages/en.json"
import type { CanvasNode } from "@/lib/types"
import { useCanvasStore } from "@/stores/canvas-store"
import {
  CanvasViewProvider,
  type CanvasViewContextValue,
} from "../canvas-view-context"
import { NoteNode } from "./note-node"

function makeCanvasNode(over: Partial<CanvasNode> = {}): CanvasNode {
  return {
    id: 101,
    kind: "note",
    folder_id: null,
    folder_group_id: null,
    agent_type: null,
    conversation_id: null,
    member_ids: [],
    title: null,
    content: "Existing note content",
    path: null,
    color: null,
    collapsed: false,
    grid_columns: 0,
    grid_rows: 0,
    x: 10,
    y: 20,
    width: 200,
    height: 140,
    created_at: "2026-09-01T00:00:00Z",
    updated_at: "2026-09-01T00:00:00Z",
    ...over,
  }
}

function createMockViewContext(
  overrides: Partial<CanvasViewContextValue> = {}
): CanvasViewContextValue {
  return {
    expandedRegions: new Set(),
    setRegionExpanded: vi.fn(),
    detailCards: new Set(),
    setCardDetail: vi.fn(),
    liveSurfaces: new Set(),
    activateSurface: vi.fn(),
    detachMember: vi.fn().mockResolvedValue(undefined),
    removeMember: vi.fn().mockResolvedValue(undefined),
    selectedConversationIds: new Set(),
    dropTargetRegionId: null,
    renamingRegionId: null,
    setRenamingRegionId: vi.fn(),
    patchNode: vi.fn().mockResolvedValue(undefined),
    endNodeResize: vi.fn(),
    deleteNode: vi.fn().mockResolvedValue(undefined),
    openConversation: vi.fn(),
    openConversationDrawer: vi.fn(),
    contextKeyForPin: vi.fn((id) => `pin-${id}`),
    draftSurfaceKey: vi.fn((id) => `draft-${id}`),
    saveSelectionAsNote: vi.fn().mockResolvedValue(undefined),
    dismissDraft: vi.fn(),
    sendingDrafts: new Set(),
    setDraftSending: vi.fn(),
    setDraftAgent: vi.fn(),
    setDraftTarget: vi.fn(),
    setDraftColor: vi.fn(),
    materializeDraft: vi.fn().mockResolvedValue(undefined),
    ...overrides,
  }
}

function renderNote(dbNode: CanvasNode, ctx = createMockViewContext()) {
  const props = {
    id: `node-${dbNode.id}`,
    data: { dbNode },
    selected: false,
    type: "note" as const,
    zIndex: 1,
    isConnectable: false,
    positionAbsoluteX: dbNode.x,
    positionAbsoluteY: dbNode.y,
    dragging: false,
    draggable: true,
    selectable: true,
    deletable: true,
  }

  const result = render(
    <NextIntlClientProvider locale="en" messages={enMessages}>
      <ReactFlowProvider>
        <CanvasViewProvider value={ctx}>
          <NoteNode {...props} />
        </CanvasViewProvider>
      </ReactFlowProvider>
    </NextIntlClientProvider>
  )
  return { ...result, ctx }
}

describe("NoteNode unmount and interaction", () => {
  beforeEach(() => {
    useCanvasStore.getState().reset()
  })

  it("renders note content at rest", () => {
    const node = makeCanvasNode({ content: "Hello World Note" })
    renderNote(node)
    expect(screen.getByText("Hello World Note")).toBeDefined()
  })

  it("enters edit mode on double click and commits on blur", async () => {
    const node = makeCanvasNode({ id: 42, content: "Initial" })
    const { ctx } = renderNote(node)

    const textEl = screen.getByText("Initial")
    fireEvent.doubleClick(textEl)

    const textarea = screen.getByRole("textbox")
    expect(textarea).toBeDefined()
    expect((textarea as HTMLTextAreaElement).value).toBe("Initial")

    fireEvent.change(textarea, { target: { value: "Updated text" } })
    fireEvent.blur(textarea)

    expect(ctx.patchNode).toHaveBeenCalledWith(42, { content: "Updated text" })
  })

  it("does not call patch on unmount if node was removed from canvas store", () => {
    const node = makeCanvasNode({ id: 99, content: "Will be deleted" })
    // Seed the store WITHOUT node 99 (simulating deletion from store)
    useCanvasStore.setState({ nodes: new Map() })

    const { ctx, unmount } = renderNote(node)

    fireEvent.doubleClick(screen.getByText("Will be deleted"))
    const textarea = screen.getByRole("textbox")
    fireEvent.change(textarea, {
      target: { value: "Pending change before delete" },
    })

    // Unmount while node is not in store
    unmount()

    expect(ctx.patchNode).not.toHaveBeenCalled()
  })

  it("calls patch on unmount if node is still present in canvas store", () => {
    const node = makeCanvasNode({ id: 99, content: "Still here" })
    // Seed the store WITH node 99
    useCanvasStore.setState({ nodes: new Map([[node.id, node]]) })

    const { ctx, unmount } = renderNote(node)

    fireEvent.doubleClick(screen.getByText("Still here"))
    const textarea = screen.getByRole("textbox")
    fireEvent.change(textarea, { target: { value: "Unmount saved text" } })

    unmount()

    expect(ctx.patchNode).toHaveBeenCalledWith(99, {
      content: "Unmount saved text",
    })
  })
})
