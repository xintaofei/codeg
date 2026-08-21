"use client"

import { useState } from "react"
import { useTranslations } from "next-intl"
import { ChevronDown } from "lucide-react"
import { Button } from "@/components/ui/button"
import {
  Dialog,
  DialogContent,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogTitle,
} from "@/components/ui/dialog"
import { Textarea } from "@/components/ui/textarea"
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "@/components/ui/collapsible"
import { workTaskCreateFromForge } from "@/lib/api"
import {
  toLocalizedErrorMessage,
  type AppErrorTranslator,
} from "@/lib/app-error"
import { useWorkbenchRoute } from "@/contexts/workbench-route-context"
import type {
  ForgeIssueRow,
  ForgeProviderId,
  ForgeRemote,
  WorkTask,
} from "@/lib/types"

/** GitLab calls it a merge request, and a dialog that promises a pull request
 *  and then opens a merge request reads like the wrong tool answered. */
function descriptionKey(isPr: boolean, provider: ForgeProviderId) {
  if (!isPr) return "dialogDescription"
  return provider === "gitlab" ? "dialogDescriptionMr" : "dialogDescriptionPr"
}

/**
 * Trigger dialog: confirm the work item, optionally add an instruction, and
 * mint the task. The snapshot preview is READ-ONLY by design — the prompt
 * (instruction wording + untrusted-data envelope) is composed server-side, so
 * there is nothing editable to show; what you see is what the envelope wraps.
 * A duplicate answer swaps the footer into "view existing / create anyway".
 */
export function ForgeStartDialog({
  row,
  remote,
  folderId,
  onClose,
  onCreated,
}: {
  row: ForgeIssueRow
  remote: ForgeRemote
  folderId: number
  onClose: () => void
  onCreated: (task: WorkTask) => void
}) {
  const t = useTranslations("Forge")
  // Root-scoped: backend errors carry FULL dotted keys the namespaced `t`
  // above cannot resolve. See `backup-settings.tsx` for the same pairing.
  const tRoot = useTranslations()
  const { setRoute } = useWorkbenchRoute()
  const [instruction, setInstruction] = useState("")
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [duplicate, setDuplicate] = useState<WorkTask | null>(null)

  const submit = async (force: boolean) => {
    setSubmitting(true)
    setError(null)
    try {
      const result = await workTaskCreateFromForge({
        folder_id: folderId,
        source: {
          kind: row.is_pr ? "pr" : "issue",
          provider: remote.provider,
          server_host: remote.server_host,
          owner_repo: remote.owner_repo,
          number: row.number,
        },
        snapshot: {
          title: row.title,
          body: row.body,
          // Names only: the snapshot is TEXT handed to an agent, and a colour
          // means nothing there.
          labels: row.labels.map((l) => l.name),
          author: row.author,
        },
        instruction: instruction.trim() || null,
        force,
      })
      switch (result.outcome) {
        case "created":
          onCreated(result.task)
          return
        case "duplicate":
          setDuplicate(result.existing)
          return
        case "folder_mismatch": {
          const remoteLabel = result.folder_remote
            ? `${result.folder_remote.server_host}/${result.folder_remote.owner_repo}`
            : "—"
          setError(t("folderMismatch", { remote: remoteLabel }))
          return
        }
      }
    } catch (e) {
      // A rejected `invoke()` throws the SERIALIZED AppCommandError object,
      // not an Error — `String(e)` on it renders "[object Object]".
      setError(
        toLocalizedErrorMessage(e, tRoot as unknown as AppErrorTranslator)
      )
    } finally {
      setSubmitting(false)
    }
  }

  return (
    <Dialog open onOpenChange={(open) => !open && onClose()}>
      <DialogContent className="sm:max-w-md">
        <DialogHeader>
          <DialogTitle className="truncate">
            #{row.number} · {row.title}
          </DialogTitle>
          <DialogDescription>
            {t(descriptionKey(row.is_pr, remote.provider), {
              repo: `${remote.server_host}/${remote.owner_repo}`,
            })}
          </DialogDescription>
        </DialogHeader>

        {duplicate == null ? (
          <>
            <Textarea
              value={instruction}
              onChange={(e) => setInstruction(e.target.value)}
              placeholder={t("instructionPlaceholder")}
              rows={3}
            />
            <Collapsible>
              <CollapsibleTrigger className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground">
                <ChevronDown className="h-3 w-3" />
                {t("previewToggle")}
              </CollapsibleTrigger>
              <CollapsibleContent>
                {/* Read-only: this is the DATA the server-side envelope wraps. */}
                <div className="mt-2 max-h-40 overflow-y-auto whitespace-pre-wrap rounded-md border border-border/60 bg-muted/40 p-2 font-mono text-[0.6875rem] text-muted-foreground">
                  {row.body?.trim() || t("previewEmpty")}
                </div>
              </CollapsibleContent>
            </Collapsible>
          </>
        ) : (
          <div className="rounded-md border border-border/60 bg-muted/40 p-3 text-sm">
            {t("duplicateBody", {
              title: duplicate.title,
              status: duplicate.status,
            })}
          </div>
        )}

        {error != null ? (
          // Announced: the trigger can be sent from the keyboard, and a
          // mismatch or a transport failure that only renders is a dead key.
          <p role="alert" className="text-xs text-destructive">
            {error}
          </p>
        ) : null}

        <DialogFooter>
          {duplicate == null ? (
            <>
              <Button variant="ghost" onClick={onClose} disabled={submitting}>
                {t("cancel")}
              </Button>
              <Button onClick={() => void submit(false)} disabled={submitting}>
                {submitting ? t("creating") : t("create")}
              </Button>
            </>
          ) : (
            <>
              <Button variant="ghost" onClick={onClose}>
                {t("cancel")}
              </Button>
              <Button
                variant="secondary"
                onClick={() => {
                  onClose()
                  setRoute("tasks")
                }}
              >
                {t("viewExisting")}
              </Button>
              <Button
                variant="secondary"
                onClick={() => void submit(true)}
                disabled={submitting}
              >
                {submitting ? t("creating") : t("createAnyway")}
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogContent>
    </Dialog>
  )
}
