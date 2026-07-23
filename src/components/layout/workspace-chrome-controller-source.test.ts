import { readFileSync } from "node:fs"
import { resolve } from "node:path"

const controllerSource = readFileSync(
  resolve(
    process.cwd(),
    "src/components/layout/workspace-chrome-controller.tsx"
  ),
  "utf8"
)

const tabBarSource = readFileSync(
  resolve(process.cwd(), "src/components/tabs/tab-bar.tsx"),
  "utf8"
)

const fileTabBarSource = readFileSync(
  resolve(process.cwd(), "src/components/files/file-workspace-tab-bar.tsx"),
  "utf8"
)

describe("tab close/navigation shortcuts live in the always-mounted controller", () => {
  // Regression guard. mod+w / mod+tab / mod+shift+tab / close-all-file-tabs
  // used to be registered by a `window` keydown listener INSIDE the visible tab
  // strips (TabBar, FileWorkspaceTabBar). The mobile workspace no longer mounts
  // those strips (it shows the folder-title header instead), and the file strip
  // is desktop-only anyway — so listeners living there would silently die below
  // the 768px breakpoint. Worse, with the listener gone, mod+w falls through to
  // the native OS default and closes the whole Tauri/browser window instead of
  // the active tab. The shortcuts must therefore live in the headless,
  // always-mounted WorkspaceChromeController (mounted on BOTH platforms).
  it("registers the tab shortcuts in WorkspaceChromeController", () => {
    expect(controllerSource).toMatch(/shortcuts\.next_tab/)
    expect(controllerSource).toMatch(/shortcuts\.prev_tab/)
    expect(controllerSource).toMatch(/shortcuts\.close_current_tab/)
    expect(controllerSource).toMatch(/shortcuts\.close_all_file_tabs/)
    // ...and actually drives the tab / file-tab actions. The e.preventDefault()
    // calls next to these are what stop mod+w reaching the window-close default.
    expect(controllerSource).toMatch(/switchTab\(/)
    expect(controllerSource).toMatch(/closeTab\(/)
    expect(controllerSource).toMatch(/closeFileTab\(/)
    expect(controllerSource).toMatch(/closeAllFileTabs\(/)
  })

  it("removes the keydown shortcut listeners from both tab strips", () => {
    // The strips are conditionally mounted (desktop only; the file strip only
    // when a file tab is open), so they must not own any global shortcut.
    expect(tabBarSource).not.toContain('addEventListener("keydown"')
    expect(tabBarSource).not.toContain("matchShortcutEvent")
    expect(fileTabBarSource).not.toContain('addEventListener("keydown"')
    expect(fileTabBarSource).not.toContain("matchShortcutEvent")
  })
})
