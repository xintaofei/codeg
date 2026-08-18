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
const globalsCssSource = readFileSync(
  resolve(process.cwd(), "src/app/globals.css"),
  "utf8"
)
const workspaceLayoutSource = readFileSync(
  resolve(process.cwd(), "src/app/workspace/layout.tsx"),
  "utf8"
)
const tabBarSource = readFileSync(
  resolve(process.cwd(), "src/components/tabs/tab-bar.tsx"),
  "utf8"
)
const messageListViewSource = readFileSync(
  resolve(process.cwd(), "src/components/message/message-list-view.tsx"),
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
    // The welcome page scrolls with the app's shared overlay scrollbar (the
    // sidebar's os-theme-codeg bar), not the platform's native one. `min-h-full`
    // on the inner column preserves the spacer layout the old
    // `overflow-y-auto` flex column had.
    expect(welcomeBranch).toContain("<ScrollArea")
    expect(welcomeBranch).toContain('className="flex min-h-full flex-col"')
    expect(welcomeBranch).not.toContain("overflow-y-auto")
    expect(welcomeBranch).not.toContain("WelcomeBackdrop")
    // The welcome input is flushed: the welcome column already supplies px-4, so
    // the input must not double-pad (would make it narrower than the cards).
    expect(welcomeBranch).toContain("flush")
    // The welcome composer is taller (min-h-30) than the compact default kept by
    // active/historical conversations.
    expect(welcomeBranch).toContain("tall")
  })

  it("snaps the hidden keep-alive tab so `transition-all` descendants don't ghost", () => {
    // Inactive tabs stay mounted and hide with `visibility: hidden` (`invisible`).
    // In Tailwind v4 `transition-all` transitions `visibility` too, so welcome
    // controls (agent pills, quick-action tabs, composer buttons) would linger
    // 150–300ms as ghosts over the newly-active conversation. The wrapper must
    // carry `conversation-tab-hidden` next to `invisible`, and globals.css must
    // drop transitions for that subtree so visibility snaps. Both halves are
    // required — assert they stay coupled.
    expect(source).toContain(
      '"conversation-tab-hidden absolute inset-0 invisible pointer-events-none"'
    )
    expect(globalsCssSource).toContain(".conversation-tab-hidden *")
    const rule = globalsCssSource.slice(
      globalsCssSource.indexOf(".conversation-tab-hidden,"),
      globalsCssSource.indexOf(".conversation-tab-hidden,") + 200
    )
    expect(rule).toContain("transition-property: none !important")
  })

  // Regression: with a workspace background image on, every covering surface is
  // TRANSPARENT rather than opaque, so a hidden-but-mounted subtree that still
  // paints is visible straight through it. `visibility` inherits, but a
  // descendant can opt back in — Monaco's DiffEditorWidget writes an inline
  // `visibility: visible` on its two panes — so an open git-diff file tab showed
  // through the full-page routes (task board / automations / token usage) and
  // through the conversation overlay in conversation-only mode, while a plain
  // file tab (no inline visibility) hid correctly.
  it("re-hides Monaco's diff panes inside a hidden keep-alive subtree", () => {
    const selector = ".conversation-tab-hidden .monaco-diff-editor > .editor"
    expect(globalsCssSource).toContain(selector)
    const rule = globalsCssSource.slice(
      globalsCssSource.indexOf(selector),
      globalsCssSource.indexOf(selector) + 120
    )
    // Only `!important` outranks Monaco's inline declaration.
    expect(rule).toContain("visibility: hidden !important")
  })

  it("marks every hidden keep-alive subtree with the hardening class", () => {
    // Under a full-page workbench route (desktop + mobile shells).
    expect(workspaceLayoutSource).toContain(
      '!isConversations && "conversation-tab-hidden invisible"'
    )
    // The FILE column under the conversation overlay — this is the one that
    // hosts git-diff tabs.
    expect(workspaceLayoutSource).toContain(
      'mode === "conversation" && "conversation-tab-hidden invisible"'
    )
    // The conversation column under the files-maximized overlay.
    expect(workspaceLayoutSource).toContain(
      'filesMaximized && "conversation-tab-hidden invisible"'
    )
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
    // The picker row is the last thing inside the composer wrapper; the
    // server-file dialog that follows it sits outside, so it anchors the slice.
    const pickerEnd = messageInputSource.indexOf(
      "{!attach.showNativePaperclip && (",
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

describe("ConversationDetailPanel split-group render model", () => {
  // The split feature's one structural invariant: group shells are FLAT
  // SIBLINGS keyed by their stable group id, positioned purely by computed
  // percentage rects. Nesting shells per layout-tree depth would reparent (and
  // remount) every live conversation view on split/unsplit/orientation
  // changes.
  it("renders group shells as flat keyed siblings from computed rects", () => {
    expect(source).toContain(
      "{orderedGroupIds.map((groupId) => renderGroupShell(groupId))}"
    )
    expect(source).toContain("const renderGroupShell = (groupId: string)")
    // A component defined inside render would change type identity every
    // render and remount its subtree — keep these plain function calls.
    expect(source).not.toContain("<RenderGroupShell")
    expect(source).not.toContain("<RenderTabWrapper")
    expect(source).toContain("key={groupId}")
    expect(source).toContain("computeRects(groupLayout)")
  })

  it("marks the active session whenever several are visible (split or tiled)", () => {
    expect(source).toContain("showActiveFlow={(isSplit || canTileG) && active}")
  })

  it("gives each split group its own strip and divider overlays only while split", () => {
    expect(source).toContain("<TabBar groupId={groupId} />")
    const handlesIdx = source.indexOf("groupHandles.map((handle) => (")
    expect(handlesIdx).toBeGreaterThan(-1)
    expect(source.slice(handlesIdx - 80, handlesIdx)).toContain("{isSplit &&")
  })

  // Each split group keeps the unsplit layout's "tabs + conversation title
  // bar" pairing: its own header (driven by the GROUP's selected tab) sits
  // under its strip, and the global single header steps aside while split.
  it("pairs every split group with its own title bar and gates the global one", () => {
    const shellStart = source.indexOf("const renderGroupShell = (groupId")
    const shellBody = source.slice(shellStart, shellStart + 6000)
    expect(shellBody).toContain("{isSplit && selTab && (")
    expect(shellBody).toContain("<ConversationDetailHeader")
    expect(shellBody).toContain("tabId={selTab.id}")
    expect(source).toContain("{!isSplit && activeTab && (")
  })

  // While split the workspace layout drops its title-bar strip row ENTIRELY —
  // no blank drag row above the shells. The window-drag surface moves into the
  // group strips instead: every strip's tail spacer is a drag region, and the
  // TOP-edge strips re-create the corner reserves (traffic lights / caption
  // buttons / chrome clusters) the unsplit row normally provides.
  it("replaces the unsplit title-bar row with in-strip drag surfaces while split", () => {
    // Layout: the whole h-10 conversation top bar is gated on !isConvSplit;
    // the old always-rendered row with a split drag-region branch is gone.
    expect(workspaceLayoutSource).toContain("{!isConvSplit && (")
    expect(workspaceLayoutSource).not.toContain("hasConvTabs && !isConvSplit")

    // Panel: TOP-edge group strips carry the corner reserves themselves.
    const shellStart = source.indexOf("const renderGroupShell = (groupId")
    const shellBody = source.slice(shellStart, shellStart + 6000)
    expect(shellBody).toContain(
      '{touchesLeft && <SplitStripCornerReserve side="left" />}'
    )
    expect(shellBody).toContain(
      '{touchesRight && <SplitStripCornerReserve side="right" />}'
    )

    // Tab bar: the tail spacer is a window-drag region on EVERY strip (group
    // strips are the window's top edge while split), not just the unsplit one.
    expect(tabBarSource).toContain(
      '<div data-tauri-drag-region className="h-full min-w-10 flex-1" />'
    )
    expect(tabBarSource).not.toContain("data-tauri-drag-region={groupId")
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

describe("ConversationDetailPanel session-load failure surface", () => {
  // When session/load fails on a conversation whose transcript already
  // rendered (e.g. its folder was deleted), the history must STAY readable;
  // the failure surfaces as a banner docked at the composer, not as a
  // full-page error over the message area.
  it("escalates the ACP load error to full-page only when nothing is renderable", () => {
    expect(messageListViewSource).toContain(
      "const blockingLoadError = hasRenderableContent ? null : (acpLoadError ?? null)"
    )
  })

  it("docks the load error at the composer with the recovery actions", () => {
    // The composer input stays hidden (a send can't reach the dead session)…
    expect(source).toContain(
      "hideInput={isWelcomeMode || Boolean(acpLoadError)}"
    )
    // …and the banner takes its place, explaining why and offering recovery.
    expect(source).toContain("composerBanner={acpLoadErrorBanner}")
    const bannerStart = source.indexOf("const acpLoadErrorBanner")
    expect(bannerStart).toBeGreaterThan(-1)
    const bannerEnd = source.indexOf("const goalControlValue", bannerStart)
    expect(bannerEnd).toBeGreaterThan(bannerStart)
    const banner = source.slice(bannerStart, bannerEnd)
    expect(banner).toContain("hasPersistedConversation && acpLoadError")
    expect(banner).toContain("handleReloadDetail")
    expect(banner).toContain("handleOpenNewSession")
    // The shell renders the banner inside the composer dock, constrained to
    // the same message-column width as the input it replaces.
    const dockIdx = conversationShellSource.indexOf("{composerBanner && (")
    expect(dockIdx).toBeGreaterThan(-1)
    const dock = conversationShellSource.slice(dockIdx, dockIdx + 200)
    expect(dock).toContain("mx-auto w-full max-w-3xl")
  })

  it("never clears a resolved session id when the persisted detail is absent", () => {
    // `externalId` is what gets handed to acp_connect, and it resolves from the
    // persisted detail OR the runtime store value the connSessionId effect
    // wrote. `detail` is null while any (re)fetch is in flight, so writing null
    // to the store in that window discards a session id we already know — and a
    // reconnect with no session id takes session/new, which is precisely how a
    // conversation's history gets stranded (codeg#500). The backend now refuses
    // to destroy the history either way; this keeps the frontend from steering
    // into it in the first place.
    const effectStart = source.indexOf(
      "if (effectiveConversationId <= 0) return"
    )
    expect(effectStart).toBeGreaterThan(-1)
    const effectEnd = source.indexOf(
      "}, [effectiveConversationId,",
      effectStart
    )
    expect(effectEnd).toBeGreaterThan(effectStart)
    const effect = source.slice(effectStart, effectEnd)

    expect(effect).toContain("const persisted = detail?.summary.external_id")
    expect(effect).toContain("if (!persisted) return")
    expect(effect).toContain(
      "setExternalId(effectiveConversationId, persisted)"
    )
    // The regression this guards: the old body passed `?? null` straight
    // through, so an in-flight refetch wiped the id.
    expect(effect).not.toContain(
      "setExternalId(effectiveConversationId, detail?.summary.external_id ?? null)"
    )
  })
})
