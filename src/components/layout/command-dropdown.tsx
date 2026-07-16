"use client"

import { useState, useEffect, useCallback, useMemo, useRef } from "react"
import { ChevronDown, Play, Plus, Square } from "lucide-react"
import { useTranslations } from "next-intl"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { useActiveFolder } from "@/contexts/active-folder-context"
import { useTerminalContext } from "@/contexts/terminal-context"
import {
  bootstrapFolderCommandsFromPackageJson,
  listFolderCommands,
  terminalKill,
} from "@/lib/api"
import type { FolderCommand } from "@/lib/types"
import { cn } from "@/lib/utils"
import { CommandManageDialog } from "./command-manage-dialog"

function getSelectedCommandId(folderId: number): number | null {
  try {
    const v = localStorage.getItem(`lastCmd:${folderId}`)
    return v ? Number(v) : null
  } catch {
    return null
  }
}

function setSelectedCommandId(folderId: number, cmdId: number) {
  try {
    localStorage.setItem(`lastCmd:${folderId}`, String(cmdId))
  } catch {
    /* ignore */
  }
}

export function CommandDropdown({ compact = false }: { compact?: boolean }) {
  const t = useTranslations("Folder.commandDropdown")
  const { activeFolder: folder } = useActiveFolder()
  const {
    createTerminalWithCommand,
    exitedTerminals,
    tabs: terminalTabs,
  } = useTerminalContext()
  const [commands, setCommands] = useState<FolderCommand[]>([])
  const [manageOpen, setManageOpen] = useState(false)
  const [bootstrapping, setBootstrapping] = useState(false)
  const [selectedCommandId, setSelectedCommandIdState] = useState<
    number | null
  >(null)
  const [runningCommandTerminals, setRunningCommandTerminals] = useState<
    Record<number, string>
  >({})
  const runningCommandTerminalsRef = useRef<Record<number, string>>({})

  const folderId = folder?.id ?? 0
  const folderPath = folder?.path ?? ""

  useEffect(() => {
    runningCommandTerminalsRef.current = runningCommandTerminals
  }, [runningCommandTerminals])

  // React to process exits reported by the terminal context
  useEffect(() => {
    if (exitedTerminals.size === 0) return
    setRunningCommandTerminals((prev) => {
      if (Object.keys(prev).length === 0) return prev
      let changed = false
      const next = { ...prev }
      for (const [cmdId, termId] of Object.entries(prev)) {
        if (exitedTerminals.has(termId)) {
          delete next[Number(cmdId)]
          changed = true
        }
      }
      return changed ? next : prev
    })
  }, [exitedTerminals])

  // React to terminal tabs being closed (e.g. user closes the tab directly)
  useEffect(() => {
    setRunningCommandTerminals((prev) => {
      if (Object.keys(prev).length === 0) return prev
      const tabIds = new Set(terminalTabs.map((t) => t.id))
      let changed = false
      const next = { ...prev }
      for (const [cmdId, termId] of Object.entries(prev)) {
        if (!tabIds.has(termId)) {
          delete next[Number(cmdId)]
          changed = true
        }
      }
      return changed ? next : prev
    })
  }, [terminalTabs])

  const selectCommand = useCallback(
    (commandId: number) => {
      if (!folderId) return
      setSelectedCommandId(folderId, commandId)
      setSelectedCommandIdState(commandId)
    },
    [folderId]
  )

  useEffect(() => {
    if (!folderId) {
      setSelectedCommandIdState(null)
      return
    }
    setSelectedCommandIdState(getSelectedCommandId(folderId))
  }, [folderId])

  // Monotonic epoch guarding async command loads: bumped on every folder change
  // (load effect below) and on every refreshCommands call, so a slow response
  // for a previous folder or a superseded refresh can't overwrite newer state.
  const loadEpochRef = useRef(0)

  const refreshCommands = useCallback(async () => {
    if (!folderId) return
    const epoch = (loadEpochRef.current += 1)
    try {
      const list = await listFolderCommands(folderId)
      if (epoch !== loadEpochRef.current) return
      setCommands(list)
    } catch (err) {
      console.error("Failed to load commands:", err)
    }
  }, [folderId])

  useEffect(() => {
    if (!folderId) return
    loadEpochRef.current += 1
    let ignore = false
    const loadCommands = async () => {
      try {
        setBootstrapping(false)
        const data = await listFolderCommands(folderId)
        if (ignore) return

        if (data.length > 0 || !folderPath) {
          setCommands(data)
          return
        }

        setBootstrapping(true)
        const bootstrapped = await bootstrapFolderCommandsFromPackageJson(
          folderId,
          folderPath
        )
        if (!ignore) setCommands(bootstrapped)
      } catch (err) {
        console.error("Failed to load commands:", err)
      } finally {
        if (!ignore) setBootstrapping(false)
      }
    }

    loadCommands()

    return () => {
      ignore = true
    }
  }, [folderId, folderPath])

  const runCommand = useCallback(
    async (cmd: FolderCommand) => {
      if (!folderPath) return
      if (runningCommandTerminalsRef.current[cmd.id]) return

      selectCommand(cmd.id)
      const terminalId = await createTerminalWithCommand(cmd.name, cmd.command)
      if (!terminalId) return

      setRunningCommandTerminals((prev) => ({ ...prev, [cmd.id]: terminalId }))
    },
    [createTerminalWithCommand, folderPath, selectCommand]
  )

  const stopCommand = useCallback(async (cmd: FolderCommand) => {
    const terminalId = runningCommandTerminalsRef.current[cmd.id]
    if (!terminalId) return

    setRunningCommandTerminals((prev) => {
      if (!(cmd.id in prev)) return prev
      const next = { ...prev }
      delete next[cmd.id]
      return next
    })
    try {
      await terminalKill(terminalId)
    } catch (err) {
      console.error("Failed to stop command terminal:", err)
    }
  }, [])

  const activeCmd = useMemo(
    () =>
      commands.find((c) => c.id === selectedCommandId) ?? commands[0] ?? null,
    [commands, selectedCommandId]
  )
  const activeTerminalId = activeCmd
    ? runningCommandTerminals[activeCmd.id]
    : undefined
  const isActiveCommandRunning = Boolean(activeTerminalId)

  useEffect(() => {
    if (!activeCmd && selectedCommandId !== null) {
      setSelectedCommandIdState(null)
      return
    }
    if (!activeCmd || selectedCommandId === activeCmd.id) return
    selectCommand(activeCmd.id)
  }, [activeCmd, selectedCommandId, selectCommand])

  const handleRunOrStop = useCallback(() => {
    if (!activeCmd) return
    if (isActiveCommandRunning) {
      void stopCommand(activeCmd)
      return
    }
    void runCommand(activeCmd)
  }, [activeCmd, isActiveCommandRunning, runCommand, stopCommand])

  const handleSelectCommand = useCallback(
    (cmd: FolderCommand) => {
      selectCommand(cmd.id)
    },
    [selectCommand]
  )

  if (!folder) return null

  // The trigger varies with command count, but the manage dialog is rendered
  // once outside the branch so saving the first command (which flips the
  // trigger from the add-button to the split-button) never remounts and closes
  // the dialog mid-edit.
  return (
    <>
      {commands.length === 0 ? (
        // No commands → show add command button
        <Button
          variant="ghost"
          size={compact ? "icon" : "sm"}
          className={cn(
            "text-xs gap-1 hover:text-foreground/80",
            compact ? "h-11 w-11 rounded-xl" : "h-6 px-2"
          )}
          onClick={() => setManageOpen(true)}
          disabled={bootstrapping}
          title={bootstrapping ? t("loading") : t("addCommand")}
        >
          <Plus className={cn("h-3 w-3", compact && "h-5 w-5")} />
          <span className={cn(compact && "sr-only")}>
            {bootstrapping ? t("loading") : t("addCommand")}
          </span>
        </Button>
      ) : (
        // Has commands → split button: [name ▼] [run/stop]
        <div className="flex min-w-0 items-center">
          <DropdownMenu>
            <DropdownMenuTrigger asChild>
              <Button
                variant="ghost"
                className={cn(
                  "hover:text-foreground/80",
                  compact ? "h-11 max-w-20 gap-1 px-2" : "h-6"
                )}
              >
                <span
                  className={cn("truncate", compact ? "max-w-12" : "max-w-24")}
                >
                  {activeCmd?.name}
                </span>
                <ChevronDown className="h-3 w-3" />
              </Button>
            </DropdownMenuTrigger>
            <DropdownMenuContent align="end" className="min-w-56">
              {commands.map((cmd) => (
                <DropdownMenuItem
                  key={cmd.id}
                  onClick={() => handleSelectCommand(cmd)}
                  className={`flex items-center justify-between gap-4 ${
                    cmd.id === activeCmd?.id ? "bg-accent/60" : ""
                  }`}
                >
                  <span className="truncate">{cmd.name}</span>
                  <span className="text-xs text-muted-foreground font-mono truncate max-w-32">
                    {cmd.command}
                  </span>
                </DropdownMenuItem>
              ))}
              <DropdownMenuSeparator />
              <DropdownMenuItem onClick={() => setManageOpen(true)}>
                {t("manageCommands")}
              </DropdownMenuItem>
            </DropdownMenuContent>
          </DropdownMenu>
          <Button
            variant="ghost"
            size="sm"
            className={cn(
              "text-xs gap-1",
              compact ? "h-11 w-11 rounded-xl px-0" : "h-6 px-2",
              isActiveCommandRunning
                ? "text-destructive hover:text-destructive"
                : "hover:text-foreground/80"
            )}
            onClick={handleRunOrStop}
            title={
              isActiveCommandRunning
                ? t("stopCommandTitle", { command: activeCmd?.command ?? "" })
                : t("runCommandTitle", { command: activeCmd?.command ?? "" })
            }
          >
            {isActiveCommandRunning ? (
              <Square className="h-3 w-3" />
            ) : (
              <Play className="h-3 w-3" />
            )}
          </Button>
        </div>
      )}

      <CommandManageDialog
        open={manageOpen}
        onOpenChange={setManageOpen}
        folderId={folderId}
        onChanged={refreshCommands}
      />
    </>
  )
}
