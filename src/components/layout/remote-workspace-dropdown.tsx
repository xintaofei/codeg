"use client"

import { useCallback, useState } from "react"
import { MonitorCloud, Settings } from "lucide-react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"
import {
  listRemoteWorkspaceConnections,
  openRemoteWorkspace,
} from "@/lib/remote-workspace"
import { toErrorMessage } from "@/lib/app-error"
import type { RemoteWorkspaceConnection } from "@/lib/types"
import { isDesktop } from "@/lib/platform"
import { Button } from "@/components/ui/button"
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu"
import { RemoteWorkspaceManageDialog } from "./remote-workspace-manage-dialog"

export function RemoteWorkspaceDropdown({
  // Default keeps the original mobile look (this component is shared with the
  // mobile FolderTitleBar). The desktop LeftEdgeChrome passes a darker-hover
  // variant so the button is visible against its bg-muted strip.
  triggerClassName = "h-6 w-6 hover:text-foreground/80",
}: {
  triggerClassName?: string
} = {}) {
  const t = useTranslations("RemoteWorkspace")
  const [connections, setConnections] = useState<RemoteWorkspaceConnection[]>(
    []
  )
  const [manageOpen, setManageOpen] = useState(false)

  const refresh = useCallback(async () => {
    if (!isDesktop()) return
    try {
      setConnections(await listRemoteWorkspaceConnections())
    } catch (err) {
      toast.error(t("loadFailed"), { description: toErrorMessage(err) })
    }
  }, [t])

  if (!isDesktop()) return null

  return (
    <>
      <DropdownMenu onOpenChange={(open) => open && void refresh()}>
        <DropdownMenuTrigger asChild>
          <Button
            variant="ghost"
            size="icon"
            className={triggerClassName}
            title={t("openRemoteWorkspace")}
          >
            <MonitorCloud className="h-3.5 w-3.5" />
          </Button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="start" className="w-72">
          {connections.length === 0 ? (
            <div className="px-3 py-2 text-sm text-muted-foreground">
              {t("empty")}
            </div>
          ) : (
            connections.map((connection) => (
              <DropdownMenuItem
                key={connection.id}
                onClick={() => {
                  openRemoteWorkspace(connection.id).catch((err) => {
                    toast.error(t("openFailed"), {
                      description: toErrorMessage(err),
                    })
                  })
                }}
              >
                <MonitorCloud className="h-3.5 w-3.5" />
                <span className="min-w-0">
                  <span className="block truncate">{connection.name}</span>
                  <span className="block truncate text-xs text-muted-foreground">
                    {connection.base_url}
                  </span>
                </span>
              </DropdownMenuItem>
            ))
          )}
          <DropdownMenuSeparator />
          <DropdownMenuItem onClick={() => setManageOpen(true)}>
            <Settings className="h-3.5 w-3.5" />
            {t("manage")}
          </DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
      <RemoteWorkspaceManageDialog
        open={manageOpen}
        onOpenChange={setManageOpen}
        onChanged={refresh}
      />
    </>
  )
}
