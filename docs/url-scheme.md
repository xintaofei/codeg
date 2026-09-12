# URL scheme

The desktop app registers the custom URL scheme `codeg://` so another program
can bring Codeg forward and open a specific conversation.

This is the OS handler for the same `codeg://session/<id>` form already used
as an in-app markdown mention. Mention badges (`codeg://agent/…`,
`codeg://commit/…`, `codeg://embedded/…`) stay in-process and are **not**
OS navigation.

## Forms

| URL | Effect |
| --- | --- |
| `codeg://session/214` | Open conversation **214** (Codeg's numeric id) |
| `codeg://session/<external-id>` | Open by the agent's own session id (Grok UUID, Codex thread id, …) |
| `codeg://workspace?conversationId=214` | Same lookup. `folderId` and `agent` are optional; when omitted they are read from the row |
| `codeg://workspace?folderId=3&conversationId=214&agent=grok` | Same, but rejected if folder or agent do not match the row |
| `codeg://open` / `codeg://` | Show the workspace, no tab change |

A missing or deleted conversation is a no-op besides showing the workspace.

## Examples

```bash
# macOS / Linux
open "codeg://session/214"
xdg-open "codeg://session/214"

# Windows
start codeg://session/214
```

From a local web app (the custom scheme cannot be `fetch`'d; assign it):

```js
window.location.href = "codeg://session/214"
```

## Cold start vs already running

- **Already running:** the deep link is delivered to the live process (macOS
  Apple Event, or Windows/Linux argv through the single-instance plugin). The
  main window is shown and `workspace://focus-conversation` opens the tab
  without reloading.
- **Cold start:** the URL is resolved against the database and the main
  window loads `/workspace?folderId=…&conversationId=…&agent=…`. The existing
  `DeepLinkBootstrap` then opens the tab after folders and tabs hydrate.

The scheme is registered by the desktop installer (`CFBundleURLTypes` on
macOS, protocol handler on Windows, `x-scheme-handler/codeg` on Linux). It
is not available in `codeg-server` / browser-only mode — use the
`/workspace?folderId=&conversationId=&agent=` query string there.
