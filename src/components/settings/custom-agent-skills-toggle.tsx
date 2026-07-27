"use client"

/**
 * Skills declaration toggle on a custom agent's settings card.
 *
 * codeg cannot detect where an arbitrary ACP agent loads skills from, so the
 * user declares that the agent reads the shared `.agents/skills` store. The
 * declaration lives on the stored definition (`skills_shared_store`), which is
 * why flipping it re-saves the whole definition rather than writing a separate
 * preference: hydration re-publishes the registry and every skills surface
 * (custom skills, experts, office, science) follows from the same gate.
 *
 * Registry-added agents default to off — this toggle is their only way in, the
 * add dialog's checkbox only covers the manual form.
 */

import { useCallback, useEffect, useRef, useState } from "react"
import { useTranslations } from "next-intl"
import { toast } from "sonner"

import { Switch } from "@/components/ui/switch"
import {
  acpListCustomAgents,
  acpSaveCustomAgent,
  type CustomAgentInfo,
  type CustomDistributionKind,
} from "@/lib/api"

interface CustomAgentSkillsToggleProps {
  registryId: string
}

export function CustomAgentSkillsToggle({
  registryId,
}: CustomAgentSkillsToggleProps) {
  const t = useTranslations("AcpAgentSettings")
  const [info, setInfo] = useState<CustomAgentInfo | null>(null)
  const [saving, setSaving] = useState(false)
  const mountedRef = useRef(true)

  useEffect(() => {
    mountedRef.current = true
    let cancelled = false
    acpListCustomAgents()
      .then((list) => {
        if (cancelled) return
        setInfo(list.find((a) => a.registryId === registryId) ?? null)
      })
      .catch(() => {
        // Leave the row hidden; the surrounding card already surfaces
        // list/load errors.
      })
    return () => {
      cancelled = true
      mountedRef.current = false
    }
  }, [registryId])

  const handleToggle = useCallback(
    async (next: boolean) => {
      if (!info) return
      setSaving(true)
      // Optimistic: the switch reflects the target state while the save runs
      // and snaps back on failure.
      setInfo({ ...info, skillsSharedStore: next })
      try {
        await acpSaveCustomAgent({
          registryId: info.registryId,
          name: info.name,
          description: info.description,
          version: info.version,
          distributionKind: info.distributionKind as CustomDistributionKind,
          spec: info.spec,
          iconUrl: info.iconUrl,
          skillsSharedStore: next,
        })
      } catch (err) {
        if (mountedRef.current) {
          setInfo({ ...info, skillsSharedStore: !next })
        }
        toast.error(err instanceof Error ? err.message : String(err))
      } finally {
        if (mountedRef.current) setSaving(false)
      }
    },
    [info]
  )

  if (!info) return null

  return (
    <div className="space-y-3 rounded-md border bg-muted/10 p-3">
      <div className="flex items-start justify-between gap-3">
        <div>
          <label className="text-xs font-medium">
            {t("customAgentSkillsLabel")}
          </label>
          <p className="mt-1 text-[11px] text-muted-foreground">
            {t("customAgentSkillsHint")}
          </p>
        </div>
        <Switch
          checked={info.skillsSharedStore}
          disabled={saving}
          onCheckedChange={(v) => void handleToggle(v)}
        />
      </div>
    </div>
  )
}
