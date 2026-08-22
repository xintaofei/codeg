"use client"

import { useCallback, useEffect, useMemo, useRef, useState } from "react"
import { usePathname, useRouter, useSearchParams } from "next/navigation"
import { useTranslations } from "next-intl"
import { FolderOpen, RefreshCw } from "lucide-react"
import { toast } from "sonner"

import { Button } from "@/components/ui/button"
import {
  Select,
  SelectContent,
  SelectItem,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select"
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs"
import { ExpertsBody } from "@/components/settings/experts-settings"
import { ScienceBody } from "@/components/settings/science-settings"
import { OfficeToolsBody } from "@/components/settings/office-tools-settings"
import { CustomSkillsBody } from "@/components/settings/custom-skills-settings"
import { expertsOpenCentralDir, loadFolderHistory, openFolder } from "@/lib/api"
import { revealItemInDir } from "@/lib/platform"
import { getActiveRemoteConnectionId, isDesktop } from "@/lib/transport"
import { toErrorMessage } from "@/lib/app-error"
import type { AgentSkillScope, FolderHistoryEntry } from "@/lib/types"

type SkillPackTab = "experts" | "science" | "office" | "custom"
const GLOBAL_SCOPE_VALUE = "__global__"

function normalizeTab(raw: string | null): SkillPackTab {
  return raw === "science" || raw === "office" || raw === "custom"
    ? raw
    : "experts"
}

// The Tabs primitive's built-in active styling uses a bare `data-active:`
// variant that is a no-op with Radix (which emits `data-state="active"`), so
// the visible active pill must be re-declared here per trigger.
const TRIGGER_ACTIVE =
  "data-[state=active]:bg-background data-[state=active]:text-foreground data-[state=active]:shadow-sm"

/**
 * Unified "Skill Packs" settings hub — a shared generic header + a fixed
 * top-right toolbar over a tabbed switcher for the three curated,
 * codeg-managed skill bundles (Experts, Science, Office). Each tab renders the
 * existing per-pack body verbatim.
 */
export function SkillPacksSettings() {
  const t = useTranslations("SkillPacksSettings")
  const router = useRouter()
  const pathname = usePathname()
  const searchParams = useSearchParams()

  // Local state is the visual source of truth so the tab always switches (even
  // on the Windows hard-nav runtime); the `?tab=` param is a best-effort mirror
  // for deep-linking, seeded once from the URL.
  const [tab, setTab] = useState<SkillPackTab>(() =>
    normalizeTab(searchParams.get("tab"))
  )
  const [scopeValue, setScopeValue] = useState(GLOBAL_SCOPE_VALUE)
  const [folders, setFolders] = useState<FolderHistoryEntry[]>([])

  useEffect(() => {
    let cancelled = false
    loadFolderHistory()
      .then((list) => {
        if (!cancelled) setFolders(list)
      })
      .catch((err) => {
        console.error("[SkillPacksSettings] loadFolderHistory failed:", err)
      })
    return () => {
      cancelled = true
    }
  }, [])

  const scope: AgentSkillScope =
    scopeValue === GLOBAL_SCOPE_VALUE ? "global" : "project"
  const workspacePath = scope === "project" ? scopeValue : null
  const scopeControl = useMemo(
    () => (
      <Select value={scopeValue} onValueChange={setScopeValue}>
        <SelectTrigger
          className="h-8 w-[min(360px,42vw)] justify-between text-left"
          aria-label={t("scope.label")}
        >
          <SelectValue />
        </SelectTrigger>
        <SelectContent align="end">
          <SelectItem value={GLOBAL_SCOPE_VALUE}>
            {t("scope.global")}
          </SelectItem>
          {folders.length === 0 ? (
            <div className="px-2 py-1.5 text-xs text-muted-foreground">
              {t("scope.noFolders")}
            </div>
          ) : (
            folders.map((folder) => (
              <SelectItem key={folder.path} value={folder.path}>
                <span className="flex min-w-0 items-center gap-2 text-left">
                  <span className="shrink-0 text-xs font-medium">
                    {folder.name}
                  </span>
                  <span className="truncate text-[10px] text-muted-foreground">
                    {folder.path}
                  </span>
                </span>
              </SelectItem>
            ))
          )}
        </SelectContent>
      </Select>
    ),
    [folders, scopeValue, t]
  )

  // The currently-mounted pack body publishes its reload handler here, so the
  // fixed "Refresh" button drives whichever tab is visible without remounting
  // it (which would tear down Office's in-flight install stream). Exactly one
  // body is mounted at a time, so the last registration always wins.
  const activeRefreshRef = useRef<(() => void) | null>(null)
  const registerRefresh = useCallback((fn: () => void) => {
    activeRefreshRef.current = fn
  }, [])

  const handleTabChange = useCallback(
    (next: string) => {
      const value = normalizeTab(next)
      setTab(value)
      // Preserve every other param (esp. ?remoteConnectionId=N) when mirroring
      // the active tab into the URL.
      const params = new URLSearchParams(searchParams.toString())
      params.set("tab", value)
      router.replace(`${pathname}?${params.toString()}`)
    },
    [router, pathname, searchParams]
  )

  // All three packs link from ONE shared central store (~/.codeg/skills), so a
  // single, tab-independent "open central folder" is correct for every tab.
  const handleOpenCentralDir = useCallback(async () => {
    try {
      const path = await expertsOpenCentralDir()
      if (isDesktop() && getActiveRemoteConnectionId() === null) {
        // Desktop: reveal the folder. `revealItemInDir` (not `openPath`) is
        // deliberate — the opener plugin's scope rejects `openPath` for the
        // hidden `~/.codeg/...` path.
        await revealItemInDir(path)
      } else {
        await openFolder(path)
      }
    } catch (err) {
      toast.error(t("toasts.openFolderFailed"), {
        description: toErrorMessage(err),
      })
    }
  }, [t])

  return (
    <div className="h-full flex flex-col p-3 md:p-4">
      <div className="shrink-0 pb-4">
        <h2 className="text-base font-semibold">{t("title")}</h2>
        <p className="text-xs text-muted-foreground mt-1">{t("description")}</p>
      </div>

      <Tabs
        value={tab}
        onValueChange={handleTabChange}
        className="flex flex-col flex-1 min-h-0"
      >
        <div className="flex items-center justify-between gap-3 shrink-0">
          <TabsList className="w-fit">
            <TabsTrigger value="experts" className={TRIGGER_ACTIVE}>
              {t("tabs.experts")}
            </TabsTrigger>
            <TabsTrigger value="science" className={TRIGGER_ACTIVE}>
              {t("tabs.science")}
            </TabsTrigger>
            <TabsTrigger value="office" className={TRIGGER_ACTIVE}>
              {t("tabs.office")}
            </TabsTrigger>
            <TabsTrigger value="custom" className={TRIGGER_ACTIVE}>
              {t("tabs.custom")}
            </TabsTrigger>
          </TabsList>

          {/* Fixed toolbar — identical on every tab. */}
          <div className="flex items-center gap-2 shrink-0">
            <Button
              size="sm"
              variant="outline"
              onClick={() => {
                handleOpenCentralDir().catch((err) => {
                  console.error(
                    "[SkillPacksSettings] open central dir failed:",
                    err
                  )
                })
              }}
            >
              <FolderOpen className="h-3.5 w-3.5" />
              {t("actions.openCentralDir")}
            </Button>
            <Button
              size="sm"
              variant="outline"
              onClick={() => activeRefreshRef.current?.()}
            >
              <RefreshCw className="h-3.5 w-3.5" />
              {t("actions.refresh")}
            </Button>
          </div>
        </div>

        <TabsContent
          value="experts"
          className="mt-0 flex-1 min-h-0 flex flex-col"
        >
          <ExpertsBody
            onRegisterRefresh={registerRefresh}
            scope={scope}
            workspacePath={workspacePath}
            scopeControl={scopeControl}
          />
        </TabsContent>
        <TabsContent
          value="science"
          className="mt-0 flex-1 min-h-0 flex flex-col"
        >
          <ScienceBody
            onRegisterRefresh={registerRefresh}
            scope={scope}
            workspacePath={workspacePath}
            scopeControl={scopeControl}
          />
        </TabsContent>
        <TabsContent
          value="office"
          className="mt-0 flex-1 min-h-0 flex flex-col"
        >
          <OfficeToolsBody
            onRegisterRefresh={registerRefresh}
            scope={scope}
            workspacePath={workspacePath}
            scopeControl={scopeControl}
          />
        </TabsContent>
        <TabsContent
          value="custom"
          className="mt-0 flex-1 min-h-0 flex flex-col"
        >
          <CustomSkillsBody
            onRegisterRefresh={registerRefresh}
            scope={scope}
            workspacePath={workspacePath}
            scopeControl={scopeControl}
          />
        </TabsContent>
      </Tabs>
    </div>
  )
}
