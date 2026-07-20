"use client"

import { useTerminalContext } from "@/contexts/terminal-context"
import { TerminalTabBar } from "./terminal-tab-bar"
import { TerminalView } from "./terminal-view"

export function TerminalPanel() {
  const { isOpen, tabs, activeTabId, markTerminalExited } = useTerminalContext()

  return (
    <section
      data-terminal-panel-region="true"
      className="flex h-full min-h-0 flex-col ws-surface"
    >
      <TerminalTabBar />
      <div className="relative flex-1 min-h-0 overflow-hidden">
        {tabs.map((tab) => (
          <TerminalView
            key={tab.id}
            terminalId={tab.id}
            workingDir={tab.workingDir}
            shell={tab.shell}
            initialCommand={tab.initialCommand}
            isActive={tab.id === activeTabId}
            isVisible={isOpen}
            onProcessExited={markTerminalExited}
          />
        ))}
      </div>
    </section>
  )
}
