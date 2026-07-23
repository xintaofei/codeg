import { readFileSync } from "node:fs"
import { resolve } from "node:path"

const source = readFileSync(
  resolve(
    process.cwd(),
    "src/components/conversations/conversation-detail-panel.tsx"
  ),
  "utf8"
)
const welcomeHeroSource = readFileSync(
  resolve(process.cwd(), "src/components/chat/welcome-hero.tsx"),
  "utf8"
)
const chatInputSource = readFileSync(
  resolve(process.cwd(), "src/components/chat/chat-input.tsx"),
  "utf8"
)
const messageInputSource = readFileSync(
  resolve(process.cwd(), "src/components/chat/message-input.tsx"),
  "utf8"
)
const conversationShellSource = readFileSync(
  resolve(process.cwd(), "src/components/chat/conversation-shell.tsx"),
  "utf8"
)

describe("ConversationDetailPanel new conversation layout", () => {
  it("keeps the new-conversation input in the welcome panel with the original scroll layout", () => {
    expect(source).toContain(
      "hideInput={isWelcomeMode || Boolean(acpLoadError)}"
    )

    const welcomeBranchStart = source.indexOf("{isWelcomeMode ? (")
    const nextBranchStart = source.indexOf(
      ") : showDraftHeader ?",
      welcomeBranchStart
    )

    expect(welcomeBranchStart).toBeGreaterThan(-1)
    expect(nextBranchStart).toBeGreaterThan(welcomeBranchStart)

    const welcomeBranch = source.slice(welcomeBranchStart, nextBranchStart)
    expect(welcomeBranch).toContain("<ChatInput")
    expect(welcomeBranch).toContain("overflow-x-hidden overflow-y-auto")
    expect(welcomeBranch).not.toContain("WelcomeBackdrop")
    // The welcome input is flushed: the welcome column already supplies px-4, so
    // the input must not double-pad (would make it narrower than the cards).
    expect(welcomeBranch).toContain("flush")
    // The welcome composer is taller (min-h-30) than the compact default kept by
    // active/historical conversations.
    expect(welcomeBranch).toContain("tall")
  })

  it("does not render a decorative welcome backdrop", () => {
    expect(welcomeHeroSource).not.toContain("export function WelcomeBackdrop")
    expect(welcomeHeroSource).not.toContain("bg-gradient-to-r")
  })

  it("uses the shared attached folder branch picker treatment for all chat inputs", () => {
    expect(source).not.toContain("attachFolderBranchPickerToInput")
    expect(conversationShellSource).not.toContain(
      "attachFolderBranchPickerToInput"
    )
    expect(messageInputSource).not.toContain("attachFolderBranchPickerToInput")
    expect(messageInputSource).toContain(
      "const folderBranchPickerAttached = hasFolderBranchPicker"
    )
    expect(messageInputSource).not.toContain("rounded-b-none")

    const pickerStart = messageInputSource.indexOf(
      "{hasFolderBranchPicker && ("
    )
    const pickerEnd = messageInputSource.indexOf(
      "<ImagePreviewDialog",
      pickerStart
    )
    expect(pickerStart).toBeGreaterThan(-1)
    expect(pickerEnd).toBeGreaterThan(pickerStart)

    const pickerWrapper = messageInputSource.slice(pickerStart, pickerEnd)
    expect(messageInputSource).toContain(
      '"overflow-hidden rounded-xl transition-colors"'
    )
    expect(messageInputSource).not.toContain("bg-muted/60")
    expect(messageInputSource).toContain(': "contents"')
    // The rounded border lives in the always-on base (so the active-session flow
    // gradient can overlay a real 1px border without a layout shift); the
    // attached folder-branch-picker treatment still adds a solid surface
    // (`bg-background`, which goes transparent to reveal a workspace-bg image via
    // `ws-transparent-bg` instead of frosting) + the inset focus ring on top.
    // The resting border is `border-foreground/20` (a touch darker than the
    // near-invisible default `border-input`, and legible over a background image).
    expect(messageInputSource).toContain(
      "rounded-xl border border-foreground/20 bg-transparent transition-colors"
    )
    expect(messageInputSource).toContain(
      '"bg-background ws-transparent-bg focus-within:border-ring focus-within:ring-[3px] focus-within:ring-inset focus-within:ring-ring/50"'
    )
    expect(pickerWrapper).not.toContain("border-t border-input")
    expect(pickerWrapper).not.toContain("bg-muted/30")
    expect(pickerWrapper).toContain("pt-1")
    expect(pickerWrapper).not.toContain("py-1")
    expect(pickerWrapper).toContain("rounded-b-xl")
    // The row only renders while attached below the composer, so the detached
    // `mt-1.5` else-branch is gone; it always takes the rounded-bottom box.
    expect(pickerWrapper).not.toContain("mt-1.5")
    // `px-2` keeps the left gutter aligned with the composer above while also
    // padding the trailing edge where the status indicators sit.
    expect(pickerWrapper).toContain("px-2")
    expect(pickerWrapper).not.toContain("pl-[")
    expect(pickerWrapper).not.toContain("pl-1.5")
    expect(pickerWrapper).not.toMatch(/\bborder-b\b/)
    expect(pickerWrapper).not.toMatch(/\bborder-x\b/)
    // The context-usage circle + agent connection status moved here from the
    // bottom status bar: they right-align at the trailing edge (justify-between)
    // while the folder/branch pickers stay on the left.
    expect(pickerWrapper).toContain("justify-between")
    expect(pickerWrapper).toContain("<ComposerContextUsage")
    expect(pickerWrapper).toContain("<ComposerConnectionStatus")
  })

  it("keeps ordinary chat input constrained to the message column width", () => {
    expect(conversationShellSource).toContain(
      'className="mx-auto w-full max-w-3xl"'
    )
    // Ordinary (active/historical) chat input keeps its own px-4 gutter to align
    // with the sibling cards in conversation-shell AND a tight bottom gap (pb-1)
    // matching the attached folder/branch row's `pt-1` top gap; only the welcome
    // input drops the gutter via `flush` (the welcome column already provides
    // px-4) and uses the same pb-1.
    expect(chatInputSource).toContain(
      'cn("pt-0", flush ? "pb-1" : "px-4 pb-1")'
    )
    expect(chatInputSource).toContain(
      'cn(tall ? "min-h-30" : "min-h-24", "max-h-60")'
    )
    expect(chatInputSource).not.toContain("containerClassName")
    expect(source).not.toContain("containerClassName")
    expect(conversationShellSource).not.toContain("containerClassName")
    expect(source).toContain("mx-auto flex w-full max-w-3xl")
  })
})

describe("ConversationDetailPanel chat-mode send path", () => {
  // Regression guard for the "first chat message gets stuck in the queue and is
  // never sent" bug: the chat first-send must NOT enqueue-and-return, it must
  // take the same inline create+bind+lifecycleSend path as a normal new
  // conversation. The old failure mode relied on the flush-on-connect engine,
  // which went dormant once the eager connection was already `connected`.
  it("does not special-case the chat first send into an enqueue-and-return branch", () => {
    // The old chat-draft early branch and its single-flight guard are gone.
    expect(source).not.toContain(
      "sendOwnTab?.isChat === true && dbConvIdRef.current == null"
    )
    expect(source).not.toContain("createChatPendingRef")
  })

  it("creates the chat row inline in the shared new-tab path and sends via lifecycleSend", () => {
    // Chat send is selected synchronously, then the SAME async block that
    // handles normal new conversations creates the row and delivers inline.
    expect(source).toContain("const chatSend = sendOwnTab?.isChat === true")
    expect(source).toContain("createChatConversation(")

    const sendStart = source.indexOf("const chatSend = sendOwnTab?.isChat")
    const sendEnd = source.indexOf(
      "createConversationPendingRef.current = false"
    )
    expect(sendStart).toBeGreaterThan(-1)
    expect(sendEnd).toBeGreaterThan(sendStart)
    const block = source.slice(sendStart, sendEnd)
    // Inline delivery (the fix) — not an mqEnqueue that defers to the queue.
    expect(block).toContain("lifecycleSend(draft, selectedModeIdArg, {")
    expect(block).not.toContain("mqEnqueue")
  })

  it("gates the chat-draft composer on a live connection (no offline compose)", () => {
    // allowOfflineCompose let the user send before connecting, which is what
    // parked the first prompt in the never-flushed queue. The composer now
    // waits for `connected` like a normal conversation.
    expect(source).not.toContain("allowOfflineCompose")
  })

  it("surfaces a non-silent error when the eager scratch-dir prepare fails", () => {
    // Without offline compose, a failed mkdir would silently disable the
    // composer forever; the eager effect must surface it instead.
    expect(source).toContain(
      'setAgentConnectError(tWelcome("prepareSessionFailed"))'
    )
  })
})

describe("ConversationDetailPanel send-path hardening", () => {
  // Guards for the production-readiness fixes from the Codex review of the
  // chat-mode work. The behavioral cores (readiness predicate, duplicate-create
  // rejection) are unit-tested in src/lib/queue-flush.test.ts; these assert they
  // are actually wired into the send path here.
  it("gates the direct send on a cwd-matched connection, not bare connected", () => {
    // A chat draft mid-reconnect can read a stale "connected" for the previous
    // cwd; sending then would hit the wrong workspace. handleSend must gate on
    // the readiness predicate (connected AND cwd matches), like the flush effect.
    expect(source).toContain("isConnectionReady(")
    expect(source).toContain("if (!connectionReady) return")
  })

  it("disables the welcome composer while connected-but-not-ready", () => {
    // The composer reads a downgraded status so its send affordance is disabled
    // during the transient mismatch window instead of inviting a rejected send.
    expect(source).toContain("composerConnStatus")
    expect(source).toContain("status={composerConnStatus}")
  })

  it("single-flights the unbound create before any optimistic mutation", () => {
    // A double-submit during the create window must be rejected BEFORE the
    // optimistic turn is appended, or it orphans a turn it can never deliver.
    expect(source).toContain("shouldRejectDuplicateCreate(")
    const guardIdx = source.indexOf("shouldRejectDuplicateCreate(")
    // The CALL site (assignment), not the function definition earlier in the file.
    const optimisticIdx = source.indexOf(
      "const optimisticTurn = buildOptimisticUserTurnFromDraft("
    )
    expect(guardIdx).toBeGreaterThan(-1)
    expect(optimisticIdx).toBeGreaterThan(guardIdx)
  })

  it("fully restores pre-send state when the create fails", () => {
    // A failed create must not strand the user behind a blank panel: drop the
    // optimistic turn, return to welcome mode, re-seed the draft, surface error.
    const catchIdx = source.indexOf(
      '"[ConversationTabView] create conversation:"'
    )
    expect(catchIdx).toBeGreaterThan(-1)
    const catchBlock = source.slice(catchIdx, catchIdx + 1500)
    expect(catchBlock).toContain("removeOptimisticTurn(")
    expect(catchBlock).toContain("setHasSentMessage(false)")
    expect(catchBlock).toContain("saveMessageInputDraft(")
    expect(catchBlock).toContain(
      'setAgentConnectError(tWelcome("createConversationFailed"))'
    )
  })
})
