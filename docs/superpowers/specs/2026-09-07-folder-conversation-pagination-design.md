# Folder Conversation Pagination Design

## Goal

Keep expanded folders manageable by showing only their six newest conversations
at first. A footer inside each folder reveals six more conversations per click.
Collapsing a folder resets that folder so its next expansion starts at six again.

This applies only to conversations in the Folders section. Pinned, Chat, and
Recent retain their existing behavior.

## Approach

Extend the existing flattened sidebar row model rather than hiding rendered DOM
rows or paging data at the backend. `buildRows` already owns folder expansion,
conversation order, worktree nesting, and virtual-list row generation, so it is
the narrowest layer that can preserve correct row indexes and sticky headers.

The sidebar component will hold a session-only visible-count map keyed by folder
ID. An absent entry means the initial page size of six. `buildRows` will receive
that map, slice each already-sorted folder bucket to its visible count, and emit
a folder-scoped load-more row when conversations remain.

## Interaction

- An expanded folder initially shows its first six conversations in the existing
  sort order. The current newest-first sorting behavior is unchanged.
- A `Load more` row appears immediately after the visible conversations when the
  folder contains more than six conversations.
- Each click increases only that folder's visible count by six.
- The footer disappears after the last conversation becomes visible.
- Changing a folder from expanded to collapsed removes its visible-count entry.
  Reopening it therefore returns to the default six conversations.
- Folder-group collapse does not reset member folders because the requested
  reset gesture is the folder header itself.
- Pinned, Chat, and Recent sections are not changed.

Delegation children remain attached to their visible parent conversation and do
not consume a folder page slot. Worktree and root sub-groups use their actual
folder IDs, so their pagination state remains independent.

## Row Model And Rendering

Add a folder load-more row variant containing the owning folder ID and the
number of hidden top-level conversations. Its row key includes the folder ID so
multiple expanded folders can each render a footer without key collisions.

The footer uses the existing sidebar row height, indentation, hover treatment,
and a Lucide down-chevron. A new localized label is added to all supported
message files. The action remains keyboard-accessible through a native button.

## State And Data Flow

1. Conversation selectors continue to sort and group all summaries in memory.
2. The sidebar passes per-folder visible counts into `buildRows`.
3. `buildRows` emits at most the requested number of top-level conversations for
   each expanded folder, followed by a footer when more remain.
4. Clicking the footer increments that folder's count and rebuilds the flat row
   array.
5. Collapsing a folder deletes its count before rebuilding the rows.

The visible-count map is not persisted. Reloading the application also returns
all folders to six, matching the transient nature of a load-more gesture.

## Verification

Pure row-model tests will verify that a folder with more than six conversations
emits six rows plus a footer and that increasing its count emits the next batch.

A component interaction test will verify the complete flow: six visible rows,
then twelve, then all remaining rows with no footer, followed by folder collapse
and re-expansion returning to six. Existing tests ensure that pinned, Chat,
Recent, sticky-header, worktree, and virtual-list behavior does not regress.
